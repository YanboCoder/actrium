//! `actr realm` — the Realm signing key holder's tool
//! (`actr/protocols/realm-profile.md` sections 7 and 8), and a member's
//! (`actr/protocols/realm-membership.md`, see [`super::realm_members`]).
//!
//! Subcommands:
//!   - `keygen`          — create a Realm signing key and its `realm_signing_keys` member
//!   - `create-request`  — sign a `create_realm` request
//!   - `sign-profile`    — sign a Realm Profile into a `publish_profile` request
//!   - `retire`          — sign the retired successor of a Realm Profile
//!   - `reissue-request` — sign a `reissue_owner_invite` request
//!   - `draft-request`   — sign a `draft_profile` request
//!   - `register-publisher` — sign a `register_publisher` request and send it
//!   - `join`, `invite`, `members`, `revoke-invite`, `revoke-client`,
//!     `remove-member`, `set-role`, `transfer-owner`, `leave` — membership
//!     requests signed with a Realm client key
//!
//! Requests are printed as JSON, to be sent with `POST` to the Actr Facility
//! Provider's `realm-management` endpoint; `register-publisher` sends its
//! own (`actr/protocols/authority-discovery.md` section 4.5).

use actr_protocol::realm_management::{ManagementOp, ManagementRequest};
use actr_protocol::realm_profile::{RealmPackage, sign_document};
use actr_protocol::{KeyStatus, RealmSigningKey, RealmSigningKeys, SIGNATURE_ALGORITHM};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use clap::{Args, Subcommand};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

use super::realm_members::{
    self, InviteArgs, InviteIdArgs, JoinArgs, KeyArgs, MemberArgs, RevokeClientArgs, SetRoleArgs,
    TransferOwnerArgs,
};
use crate::core::{Command, CommandContext, CommandResult, ComponentType};

#[derive(Args, Debug)]
pub struct RealmArgs {
    #[command(subcommand)]
    pub command: RealmCommand,
}

#[derive(Subcommand, Debug)]
pub enum RealmCommand {
    /// Create a Realm signing key and print its self-signed
    /// `realm_signing_keys` member for the domain's authority descriptor.
    Keygen(KeygenArgs),
    /// Sign a `create_realm` request.
    CreateRequest(CreateRequestArgs),
    /// Sign a Realm Profile (the draft from `create_realm`, or the current
    /// one to update) into a `publish_profile` request.
    SignProfile(SignProfileArgs),
    /// Sign the retired successor of the current Realm Profile into a
    /// `publish_profile` request.
    Retire(RetireArgs),
    /// Sign a `reissue_owner_invite` request.
    ReissueRequest(ReissueRequestArgs),
    /// Sign a `draft_profile` request for the next Realm Profile, with the
    /// Actr Facility Provider's current issuer keys; sign its reply with
    /// `sign-profile`.
    DraftRequest(ReissueRequestArgs),
    /// Sign a `register_publisher` request with a Publisher Profile signing
    /// key and send it to the Actr Facility Provider's `realm-management` endpoint.
    RegisterPublisher(RegisterPublisherArgs),
    /// Redeem a Realm invite with a new Realm client key and write its key
    /// file.
    Join(JoinArgs),
    /// Create an invite: a new member (owner, `--role`) or a new client of a
    /// member (`--member`).
    Invite(InviteArgs),
    /// List members, clients and open invites (a non-owner sees its own).
    Members(KeyArgs),
    /// Revoke an unused invite.
    RevokeInvite(InviteIdArgs),
    /// Revoke a client; this key file's own key if no key id is given.
    RevokeClient(RevokeClientArgs),
    /// Remove a member and revoke all its clients (owner).
    RemoveMember(MemberArgs),
    /// Change a member's role (owner).
    SetRole(SetRoleArgs),
    /// Make another member the owner (owner).
    TransferOwner(TransferOwnerArgs),
    /// Leave the Realm; revokes all your clients.
    Leave(KeyArgs),
}

#[derive(Args, Debug)]
pub struct KeygenArgs {
    /// Key id listed in `realm_signing_keys`
    #[arg(long, value_name = "ID")]
    pub key_id: String,
    /// Key file to write
    #[arg(long, short = 'o', value_name = "FILE")]
    pub output: PathBuf,
    /// Overwrite an existing key file
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct CreateRequestArgs {
    /// Realm signing key file from `actr realm keygen`
    #[arg(long, value_name = "FILE")]
    pub key: PathBuf,
    /// Realm name `<realm_id>@<domain>`
    #[arg(long, value_name = "REALM")]
    pub realm: String,
    /// Actr Facility Provider ID of the receiving Actr Facility Provider
    #[arg(long, value_name = "ID")]
    pub provider: String,
    /// HTTPS URL of the package the Realm runs
    #[arg(long, value_name = "URL")]
    pub package_href: String,
    /// Lowercase hex SHA-256 of the package
    #[arg(long, value_name = "HEX")]
    pub package_sha256: String,
    /// Actor type of the package manifest, `publisher/program@version`
    #[arg(long, value_name = "TYPE")]
    pub package_type: String,
    /// Opaque id of whom the request is signed for
    #[arg(long, value_name = "ID")]
    pub requester: Option<String>,
}

#[derive(Args, Debug)]
pub struct SignProfileArgs {
    /// Realm signing key file from `actr realm keygen`
    #[arg(long, value_name = "FILE")]
    pub key: PathBuf,
    /// Realm Profile JSON, or a `create_realm` reply carrying one (`-` reads
    /// stdin). A signed Profile is updated with the next sequence.
    #[arg(value_name = "PROFILE")]
    pub profile: PathBuf,
    /// New display name
    #[arg(long, value_name = "NAME")]
    pub display_name: Option<String>,
    /// New package URL (with --package-sha256 and --package-type)
    #[arg(long, value_name = "URL", requires_all = ["package_sha256", "package_type"])]
    pub package_href: Option<String>,
    /// New package SHA-256 (with --package-href)
    #[arg(long, value_name = "HEX", requires = "package_href")]
    pub package_sha256: Option<String>,
    /// New package Actor type `publisher/program@version` (with --package-href)
    #[arg(long, value_name = "TYPE", requires = "package_href")]
    pub package_type: Option<String>,
}

#[derive(Args, Debug)]
pub struct RetireArgs {
    /// Realm signing key file from `actr realm keygen`
    #[arg(long, value_name = "FILE")]
    pub key: PathBuf,
    /// Current signed Realm Profile JSON (`-` reads stdin)
    #[arg(value_name = "PROFILE")]
    pub profile: PathBuf,
}

#[derive(Args, Debug)]
pub struct ReissueRequestArgs {
    /// Realm signing key file from `actr realm keygen`
    #[arg(long, value_name = "FILE")]
    pub key: PathBuf,
    /// Realm name `<realm_id>@<domain>`
    #[arg(long, value_name = "REALM")]
    pub realm: String,
    /// Actr Facility Provider ID of the receiving Actr Facility Provider
    #[arg(long, value_name = "ID")]
    pub provider: String,
}

#[derive(Args, Debug)]
pub struct RegisterPublisherArgs {
    /// Active Publisher Profile key from `actr publisher profile keygen`
    /// (also accepts the realm keygen format)
    #[arg(long, value_name = "FILE")]
    pub key: PathBuf,
    /// Publisher ID: the normalized domain whose `/.well-known/actr` carries
    /// the Publisher Profile
    #[arg(long, value_name = "DOMAIN")]
    pub publisher: String,
    /// Actr Facility Provider ID of the receiving Actr Facility Provider
    #[arg(long, value_name = "ID")]
    pub provider: String,
    /// `realm-management` endpoint to send to, instead of the one listed in
    /// `https://<provider>/.well-known/actr`
    #[arg(long, value_name = "URL")]
    pub endpoint: Option<String>,
    /// Print the signed request instead of sending it
    #[arg(long)]
    pub print: bool,
}

/// A Realm signing key file.
#[derive(Serialize, Deserialize)]
pub struct RealmKeyFile {
    pub key_id: String,
    /// Base64 Ed25519 private key (32 bytes).
    pub private_key: String,
    /// Base64 Ed25519 public key.
    pub public_key: String,
}

impl RealmKeyFile {
    fn load(path: &Path) -> Result<(String, SigningKey)> {
        let text =
            actr_protocol::realm_membership::read_private_key_file(path, "Realm signing key")
                .map_err(anyhow::Error::msg)?;
        let file: RealmKeyFile = serde_json::from_str(&text)
            .with_context(|| format!("Not a Realm signing key file: {}", path.display()))?;
        let bytes: [u8; 32] = BASE64
            .decode(&file.private_key)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .context("private_key is not a base64 Ed25519 key")?;
        Ok((file.key_id, SigningKey::from_bytes(&bytes)))
    }
}

#[async_trait]
impl Command for RealmArgs {
    async fn execute(&self, _ctx: &CommandContext) -> Result<CommandResult> {
        let output = match &self.command {
            RealmCommand::Keygen(args) => keygen(args)?,
            RealmCommand::CreateRequest(args) => {
                let (key_id, key) = RealmKeyFile::load(&args.key)?;
                let mut request =
                    new_request(ManagementOp::CreateRealm, &args.realm, &args.provider);
                request.package = Some(RealmPackage {
                    href: args.package_href.clone(),
                    sha256: args.package_sha256.clone(),
                    r#type: args.package_type.clone(),
                });
                request.requester = args.requester.clone();
                request.sign(&key_id, &key)
            }
            RealmCommand::SignProfile(args) => {
                let package = match (&args.package_href, &args.package_sha256, &args.package_type) {
                    (Some(href), Some(sha256), Some(r#type)) => Some(RealmPackage {
                        href: href.clone(),
                        sha256: sha256.clone(),
                        r#type: r#type.clone(),
                    }),
                    _ => None,
                };
                let profile = read_profile(&args.profile)?;
                sign_profile(&args.key, profile, args.display_name.as_deref(), package)?
            }
            RealmCommand::Retire(args) => retire(&args.key, read_profile(&args.profile)?)?,
            RealmCommand::ReissueRequest(args) => {
                let (key_id, key) = RealmKeyFile::load(&args.key)?;
                new_request(
                    ManagementOp::ReissueOwnerInvite,
                    &args.realm,
                    &args.provider,
                )
                .sign(&key_id, &key)
            }
            RealmCommand::DraftRequest(args) => {
                let (key_id, key) = RealmKeyFile::load(&args.key)?;
                new_request(ManagementOp::DraftProfile, &args.realm, &args.provider)
                    .sign(&key_id, &key)
            }
            RealmCommand::RegisterPublisher(args) => register_publisher(args).await?,
            RealmCommand::Join(args) => realm_members::join(args).await?,
            RealmCommand::Invite(args) => realm_members::invite(args).await?,
            RealmCommand::Members(args) => realm_members::members(args).await?,
            RealmCommand::RevokeInvite(args) => realm_members::revoke_invite(args).await?,
            RealmCommand::RevokeClient(args) => realm_members::revoke_client(args).await?,
            RealmCommand::RemoveMember(args) => realm_members::remove_member(args).await?,
            RealmCommand::SetRole(args) => realm_members::set_role(args).await?,
            RealmCommand::TransferOwner(args) => realm_members::transfer_owner(args).await?,
            RealmCommand::Leave(args) => realm_members::leave(args).await?,
        };
        Ok(CommandResult::Success(serde_json::to_string_pretty(
            &output,
        )?))
    }

    fn required_components(&self) -> Vec<ComponentType> {
        vec![]
    }

    fn name(&self) -> &str {
        "realm"
    }

    fn description(&self) -> &str {
        "Realm signing key holder's and member's tool"
    }
}

fn keygen(args: &KeygenArgs) -> Result<Value> {
    if args.output.exists() && !args.force {
        bail!(
            "Key file already exists: {}\nUse --force to overwrite.",
            args.output.display()
        );
    }
    if let Some(parent) = args.output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let key = SigningKey::from_bytes(&rand::random::<[u8; 32]>());
    let public_key = BASE64.encode(key.verifying_key().to_bytes());
    let file = RealmKeyFile {
        key_id: args.key_id.clone(),
        private_key: BASE64.encode(key.to_bytes()),
        public_key: public_key.clone(),
    };
    write_private(
        &args.output,
        serde_json::to_string_pretty(&file)?.as_bytes(),
    )
    .with_context(|| format!("Failed to write key file: {}", args.output.display()))?;
    eprintln!(
        "Realm signing key written to {}; publish the member below as \
         `realm_signing_keys` in the domain's /.well-known/actr",
        args.output.display()
    );

    let member = RealmSigningKeys {
        sequence: 1,
        keys: vec![RealmSigningKey {
            id: args.key_id.clone(),
            algorithm: SIGNATURE_ALGORITHM.to_string(),
            public_key,
            status: KeyStatus::Active,
        }],
        rotations: None,
        signature: actr_protocol::DetachedSignature {
            key_id: args.key_id.clone(),
            algorithm: SIGNATURE_ALGORITHM.to_string(),
            value: String::new(),
        },
    };
    let mut member = serde_json::to_value(member)?;
    sign_document(&mut member, &args.key_id, &key);
    Ok(member)
}

/// Write `contents` to a file only its owner can read, from its creation on.
pub(crate) fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    // An existing file keeps its mode on open.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(contents)
}

fn new_request(op: ManagementOp, realm: &str, provider: &str) -> ManagementRequest {
    let nonce = hex::encode(rand::random::<[u8; 16]>());
    ManagementRequest::new(op, realm, provider, &nonce)
}

fn register_publisher_request(args: &RegisterPublisherArgs) -> Result<Value> {
    let (key_id, key) = RealmKeyFile::load(&args.key)?;
    let mut request = new_request(ManagementOp::RegisterPublisher, "", &args.provider);
    request.publisher = args.publisher.clone();
    Ok(request.sign(&key_id, &key))
}

/// Sign `register_publisher` and send it; returns the Actr Facility Provider's reply.
async fn register_publisher(args: &RegisterPublisherArgs) -> Result<Value> {
    let request = register_publisher_request(args)?;
    if args.print {
        return Ok(request);
    }
    let endpoint = match &args.endpoint {
        Some(endpoint) => endpoint.clone(),
        None => realm_management_endpoint(&args.provider).await?,
    };
    let response = realm_members::http_client()?
        .post(&endpoint)
        .json(&request)
        .send()
        .await
        .with_context(|| format!("Failed to send register_publisher to {endpoint}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("register_publisher refused (HTTP {status}): {body}");
    }
    serde_json::from_str(&body).context("register_publisher reply is not JSON")
}

/// The `realm-management` link of the Actr Facility Provider's authority descriptor.
async fn realm_management_endpoint(provider: &str) -> Result<String> {
    let url = format!("https://{provider}/.well-known/actr");
    let descriptor: actr_protocol::AuthorityDescriptor = realm_members::discovery_client()?
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Failed to fetch {url}"))?
        .json()
        .await
        .with_context(|| format!("{url} is not an authority descriptor"))?;
    Ok(descriptor.require_service("realm-management")?.href.clone())
}

/// The Realm Profile in `path`, or the `profile` member of a `create_realm`
/// reply.
fn read_profile(path: &Path) -> Result<Value> {
    let text = if path == Path::new("-") {
        std::io::read_to_string(std::io::stdin())?
    } else {
        std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?
    };
    let mut value: Value = serde_json::from_str(&text).context("Realm Profile is not JSON")?;
    if value.get("schema").is_none()
        && let Some(profile) = value.get_mut("profile")
    {
        value = profile.take();
    }
    if value.get("schema").and_then(Value::as_str) != Some("actr.realm/v1") {
        bail!("not an actr.realm/v1 Realm Profile");
    }
    Ok(value)
}

/// Sign `profile` into a `publish_profile` request. A signed Profile is the
/// previous one: the new one takes the next sequence. An unsigned draft
/// keeps its own.
fn sign_profile(
    key_path: &Path,
    mut profile: Value,
    display_name: Option<&str>,
    package: Option<RealmPackage>,
) -> Result<Value> {
    if profile.get("signature").is_some() {
        let sequence = profile["sequence"]
            .as_u64()
            .context("Realm Profile has no sequence")?;
        profile["sequence"] = (sequence + 1).into();
    }
    if let Some(name) = display_name {
        profile["realm"]["display_name"] = name.into();
    }
    if let Some(package) = package {
        profile["package"] = serde_json::to_value(package)?;
    }
    profile["issued_at"] = chrono::Utc::now()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        .into();

    let (key_id, key) = RealmKeyFile::load(key_path)?;
    let realm = profile["realm"]["name"]
        .as_str()
        .context("Realm Profile has no realm.name")?
        .to_string();
    let provider = profile["provider"]["id"]
        .as_str()
        .context("Realm Profile has no provider.id")?
        .to_string();
    sign_document(&mut profile, &key_id, &key);
    let mut request = new_request(ManagementOp::PublishProfile, &realm, &provider);
    request.profile = Some(profile);
    Ok(request.sign(&key_id, &key))
}

/// The retired successor of the signed `profile`, as a `publish_profile`
/// request.
fn retire(key_path: &Path, mut profile: Value) -> Result<Value> {
    if profile.get("signature").is_none() {
        bail!("retire needs the current signed Realm Profile");
    }
    profile["realm"]["status"] = "retired".into();
    if let Value::Object(members) = &mut profile {
        members.remove("package");
    }
    sign_profile(key_path, profile, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actr_protocol::realm_profile::RealmProfile;
    use serde_json::json;

    fn draft() -> Value {
        json!({
            "schema": "actr.realm/v1",
            "sequence": 1,
            "realm": { "name": "k7m2q9xh@acme.com", "display_name": "k7m2q9xh", "status": "active" },
            "package": {
                "href": "https://cdn.example/a.actr", "sha256": "ab".repeat(32),
                "type": "acme.com/notes@1.0.0",
            },
            "issuer": { "url": "https://p.example/issuer", "keys": [{
                "id": "7", "algorithm": "Ed25519", "status": "active",
                "public_key": BASE64.encode([2u8; 32]),
            }] },
            "provider": { "id": "p.example", "signaling": [], "ice": [] },
            "policy": { "membership": "invite-only" },
            "issued_at": "2026-09-25T00:00:00Z",
        })
    }

    #[test]
    fn signs_requests_and_profiles_the_actr_facility_provider_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("realm-key.json");
        let member = keygen(&KeygenArgs {
            key_id: "rs-1".into(),
            output: key_path.clone(),
            force: false,
        })
        .unwrap();
        let keys: RealmSigningKeys = serde_json::from_value(member).unwrap();
        keys.validate().unwrap();

        let reply = json!({ "realm": "k7m2q9xh@acme.com", "profile": draft() });
        let reply_path = dir.path().join("reply.json");
        std::fs::write(&reply_path, reply.to_string()).unwrap();
        let profile = read_profile(&reply_path).unwrap();
        let request = sign_profile(&key_path, profile, Some("Notes"), None).unwrap();

        let parsed = ManagementRequest::parse(request.to_string().as_bytes()).unwrap();
        parsed.verify(&keys.keys).unwrap();
        assert_eq!(parsed.op, ManagementOp::PublishProfile);
        assert_eq!(parsed.provider_id, "p.example");
        let signed = parsed.profile.unwrap();
        let first = RealmProfile::verify(signed.to_string().as_bytes(), "k7m2q9xh@acme.com", &keys)
            .unwrap();
        assert_eq!(
            (first.sequence, first.realm.display_name.as_str()),
            (1, "Notes")
        );

        // Retiring the signed Profile moves to the next sequence.
        let request = retire(&key_path, signed).unwrap();
        let parsed = ManagementRequest::parse(request.to_string().as_bytes()).unwrap();
        let last = RealmProfile::verify(
            parsed.profile.unwrap().to_string().as_bytes(),
            "k7m2q9xh@acme.com",
            &keys,
        )
        .unwrap();
        first.check_successor(&last).unwrap();
        assert_eq!(last.sequence, 2);
        assert!(last.package.is_none());

        // The key file is not overwritten by accident.
        assert!(
            keygen(&KeygenArgs {
                key_id: "rs-1".into(),
                output: key_path.clone(),
                force: false,
            })
            .is_err()
        );
        // Only its owner can read it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[tokio::test]
    async fn register_publisher_sends_a_request_the_actr_facility_provider_verifies() {
        use axum::{Json, Router, routing::post};

        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("publisher-key.json");
        let member = keygen(&KeygenArgs {
            key_id: "root-1".into(),
            output: key_path.clone(),
            force: false,
        })
        .unwrap();
        let keys: RealmSigningKeys = serde_json::from_value(member).unwrap();

        // A realm-management endpoint that verifies and echoes the request.
        let verifying_keys = keys.keys.clone();
        let app = Router::new().route(
            "/actr/realm-management",
            post(move |body: axum::body::Bytes| async move {
                let request = ManagementRequest::parse(&body).unwrap();
                request.verify(&verifying_keys).unwrap();
                Json(serde_json::json!({
                    "op": request.op,
                    "publisher": request.publisher,
                    "realm": request.realm,
                    "provider_id": request.provider_id,
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut args = RegisterPublisherArgs {
            key: key_path,
            publisher: "apps.acme.com".into(),
            provider: "p.example".into(),
            endpoint: Some(format!("http://{address}/actr/realm-management")),
            print: false,
        };
        let reply = register_publisher(&args).await.unwrap();
        assert_eq!(
            reply,
            serde_json::json!({
                "op": "register_publisher",
                "publisher": "apps.acme.com",
                "realm": "",
                "provider_id": "p.example",
            })
        );

        // --print only prints the signed request.
        args.endpoint = Some("http://127.0.0.1:9/unreachable".into());
        args.print = true;
        let printed = register_publisher(&args).await.unwrap();
        let request = ManagementRequest::parse(printed.to_string().as_bytes()).unwrap();
        request.verify(&keys.keys).unwrap();
        assert_eq!(request.op, ManagementOp::RegisterPublisher);
    }
}
