//! `actr realm join | invite | members | …` — a member's tool
//! (`actr/protocols/realm-membership.md`).
//!
//! `join` redeems a Realm invite with a new Realm client key and writes the
//! key file; the other commands sign a membership request with that key and
//! send it to `<issuer>/membership`. Replies are printed as JSON.

use actr_protocol::realm_membership::{
    MembershipOp, MembershipRequest, RealmClientKeyFile, RealmInvite, Role,
};
use actr_protocol::realm_profile::{RealmProfile, RealmStatus, realm_profile_urls};
use actr_protocol::uri::{ActrUri, parse_realm_name};
use actr_protocol::{AuthorityDescriptor, RealmSigningKeys};
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use clap::Args;
use ed25519_dalek::SigningKey;
use serde_json::Value;
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use super::realm::write_private;

fn parse_role(text: &str) -> std::result::Result<Role, String> {
    match Role::parse(text) {
        Some(Role::Owner) | None => Err("expected editor or viewer".to_string()),
        Some(role) => Ok(role),
    }
}

#[derive(Args, Debug, Clone)]
pub struct JoinArgs {
    /// Realm invite text `ri_…` (`-` reads stdin)
    #[arg(value_name = "INVITE")]
    pub invite: String,
    /// Key file to write the new Realm client key to
    #[arg(long, short = 'o', value_name = "FILE")]
    pub output: PathBuf,
    /// Issuer URL, for a Realm without a Realm Profile. Otherwise the
    /// Issuer and Actr Facility Provider come from the Realm Profile, verified with
    /// the `realm_signing_keys` of `https://<domain>/.well-known/actr`.
    #[arg(long, value_name = "URL")]
    pub issuer: Option<String>,
    /// Actr Facility Provider ID with `--issuer`; defaults to its `host[:port]`
    #[arg(long, value_name = "ID", requires = "issuer")]
    pub provider: Option<String>,
    /// Name of this client, shown to the Realm owner
    #[arg(long, value_name = "TEXT")]
    pub label: Option<String>,
    /// Overwrite an existing key file
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct KeyArgs {
    /// Realm client key file from `actr realm join`
    #[arg(long, value_name = "FILE")]
    pub key: PathBuf,
}

#[derive(Args, Debug)]
pub struct InviteArgs {
    #[command(flatten)]
    pub key: KeyArgs,
    /// Invite a new member with this role (owner only)
    #[arg(long, value_name = "ROLE", value_parser = parse_role, conflicts_with = "member", required_unless_present = "member")]
    pub role: Option<Role>,
    /// Invite a new client of this existing member
    #[arg(long, value_name = "MEMBER_ID")]
    pub member: Option<String>,
    /// Days the invite stays valid (at most 30)
    #[arg(long, value_name = "DAYS", default_value_t = 7)]
    pub days: u32,
}

#[derive(Args, Debug)]
pub struct InviteIdArgs {
    #[command(flatten)]
    pub key: KeyArgs,
    #[arg(value_name = "INVITE_ID")]
    pub invite_id: String,
}

#[derive(Args, Debug)]
pub struct RevokeClientArgs {
    #[command(flatten)]
    pub key: KeyArgs,
    /// Client to revoke; this key file's own key if absent
    #[arg(value_name = "KEY_ID")]
    pub key_id: Option<String>,
}

#[derive(Args, Debug)]
pub struct MemberArgs {
    #[command(flatten)]
    pub key: KeyArgs,
    #[arg(value_name = "MEMBER_ID")]
    pub member_id: String,
}

#[derive(Args, Debug)]
pub struct SetRoleArgs {
    #[command(flatten)]
    pub key: KeyArgs,
    #[arg(value_name = "MEMBER_ID")]
    pub member_id: String,
    #[arg(value_name = "ROLE", value_parser = parse_role)]
    pub role: Role,
}

#[derive(Args, Debug)]
pub struct TransferOwnerArgs {
    #[command(flatten)]
    pub key: KeyArgs,
    #[arg(value_name = "MEMBER_ID")]
    pub member_id: String,
    /// Your role after the transfer
    #[arg(long, value_name = "ROLE", value_parser = parse_role, default_value = "editor")]
    pub previous_owner_role: Role,
}

/// A loaded key file.
struct Member {
    file: RealmClientKeyFile,
    key: SigningKey,
}

impl Member {
    fn load(args: &KeyArgs) -> Result<Self> {
        let (file, key) = RealmClientKeyFile::read(&args.key)?;
        Ok(Self { file, key })
    }

    fn request(&self, op: MembershipOp) -> MembershipRequest {
        MembershipRequest::new(op, &self.file.realm, &self.file.provider_id, &new_nonce())
    }

    async fn send(&self, request: MembershipRequest) -> Result<Value> {
        Ok(send(&self.file.issuer, &request.sign(&self.key)).await?)
    }
}

fn new_nonce() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

/// `host[:port]` of `url`, lowercase: the Actr Facility Provider ID a Realm without
/// a Realm Profile is addressed by.
fn url_authority(url: &str) -> Result<String> {
    let parsed = url::Url::parse(url).with_context(|| format!("Invalid Issuer URL: {url}"))?;
    let host = parsed
        .host_str()
        .with_context(|| format!("Issuer URL has no host: {url}"))?;
    Ok(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
    .to_ascii_lowercase())
}

/// Why a membership request got no reply.
enum SendError {
    /// The Actr Facility Provider answered with an error: nothing changed.
    Refused(anyhow::Error),
    /// No usable answer: the request may or may not have taken effect.
    Unknown(anyhow::Error),
}

impl From<SendError> for anyhow::Error {
    fn from(error: SendError) -> Self {
        match error {
            SendError::Refused(error) | SendError::Unknown(error) => error,
        }
    }
}

/// Connect timeout of the CLI's Realm requests.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Timeout of a whole Realm request, reply included.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP client for membership and realm-management requests: with
/// timeouts, and without redirects, so a signed request (or an invite
/// secret) goes nowhere but where it was sent.
pub(super) fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("Failed to create the HTTP client")
}

/// HTTP client for public discovery documents: HTTPS only (every redirect
/// too), at most five redirects (`authority-discovery.md`), with timeouts.
pub(super) fn discovery_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .context("Failed to create the HTTP client")
}

/// Size limit of a public document (`authority-discovery.md`).
const PUBLIC_DOCUMENT_LIMIT: usize = 64 * 1024;

/// GET the public document at `url`: the body of a 2xx reply, at most
/// 64 KiB.
async fn fetch_public(client: &reqwest::Client, url: String) -> Result<Vec<u8>> {
    let mut response = client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Failed to fetch {url}"))?;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("Failed to read {url}"))?
    {
        if body.len() + chunk.len() > PUBLIC_DOCUMENT_LIMIT {
            bail!("{url} is larger than 64 KiB");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The Realm Profile of `realm` (`realm-profile.md` sections 3 and 5): the
/// domain's `realm_signing_keys` from `https://<domain>/.well-known/actr`,
/// then the first Realm Profile from its `realms` links that verifies with
/// them. `embedded`, an invite's Realm Profile, counts only if it verifies
/// with the same keys; if both do, the higher sequence is taken, and it must
/// be a valid successor of the other.
async fn realm_profile<F, Fut>(
    realm: &str,
    embedded: Option<&Value>,
    fetch: F,
) -> Result<RealmProfile>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    const HINT: &str = "a Realm without a Realm Profile is joined with --issuer";
    let uri = parse_realm_name(realm)?;
    let url = uri
        .discovery_url()
        .with_context(|| format!("Realm {realm} has no Realm Profile; {HINT}"))?;
    let document = fetch(url.clone()).await.context(HINT)?;
    let mut value: Value =
        serde_json::from_slice(&document).with_context(|| format!("{url} is not JSON"))?;
    let descriptor: AuthorityDescriptor = serde_json::from_value(value.clone())
        .with_context(|| format!("{url} is not an authority descriptor"))?;
    descriptor
        .validate_for(&ActrUri::from_domain(uri.domain())?)
        .with_context(|| format!("{url} is not a valid authority descriptor"))?;
    let member = value
        .get_mut("realm_signing_keys")
        .map(Value::take)
        .with_context(|| format!("{url} lists no realm_signing_keys"))?;
    let (keys, _) = RealmSigningKeys::accept(&member, None)
        .with_context(|| format!("{url}: realm_signing_keys"))?;

    let mut failures = Vec::new();
    let mut fetched = None;
    for url in realm_profile_urls(&descriptor, realm)? {
        let verified = match fetch(url.clone()).await {
            Ok(document) => RealmProfile::verify(&document, realm, &keys).map_err(Into::into),
            Err(error) => Err(error),
        };
        match verified {
            Ok(profile) => {
                fetched = Some(profile);
                break;
            }
            Err(error) => failures.push(format!("{url}: {error:#}")),
        }
    }
    let embedded = match embedded {
        Some(document) => {
            match RealmProfile::verify(document.to_string().as_bytes(), realm, &keys) {
                Ok(profile) => Some(profile),
                Err(error) => {
                    failures.push(format!("the invite's Realm Profile: {error}"));
                    None
                }
            }
        }
        None => None,
    };

    let profile = match (fetched, embedded) {
        (Some(a), Some(b)) => {
            let (lower, higher) = if a.sequence <= b.sequence {
                (a, b)
            } else {
                (b, a)
            };
            lower.check_successor(&higher)?;
            higher
        }
        (Some(profile), None) | (None, Some(profile)) => profile,
        (None, None) => {
            let failures = if failures.is_empty() {
                format!("{url} lists no realms location")
            } else {
                failures.join("; ")
            };
            bail!("No verified Realm Profile for {realm} ({HINT}): {failures}");
        }
    };
    if profile.realm.status == RealmStatus::Retired {
        bail!("Realm {realm} is retired");
    }
    Ok(profile)
}

/// POST `document` to `<issuer>/membership`; the reply, or its error.
async fn send(issuer: &str, document: &Value) -> std::result::Result<Value, SendError> {
    send_with(
        &http_client().map_err(SendError::Refused)?,
        issuer,
        document,
    )
    .await
}

async fn send_with(
    client: &reqwest::Client,
    issuer: &str,
    document: &Value,
) -> std::result::Result<Value, SendError> {
    let url = format!("{}/membership", issuer.trim_end_matches('/'));
    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(document.to_string())
        .send()
        .await
        .with_context(|| format!("Failed to send the membership request to {url}"))
        .map_err(SendError::Unknown)?;
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("Failed to read the membership reply from {url}"))
        .map_err(SendError::Unknown)?;
    if !status.is_success() {
        let error = anyhow::anyhow!("membership request failed (HTTP {status}): {body}");
        // A gateway can replace the reply after redemption committed. Only
        // recognized application refusals establish that this attempt failed.
        let code = serde_json::from_str::<Value>(&body).ok();
        let definitive = matches!(
            (
                status.as_u16(),
                code.as_ref().and_then(|v| v["error"].as_str())
            ),
            (400, Some("invalid_request"))
                | (
                    403,
                    Some(
                        "client_unknown"
                            | "client_revoked"
                            | "member_removed"
                            | "forbidden"
                            | "realm_unavailable"
                    )
                )
                | (404, Some("invite_unknown" | "not_found"))
                | (
                    409,
                    Some(
                        "invite_realm_mismatch"
                            | "owner_last_client"
                            | "owner_must_transfer"
                            | "conflict"
                    )
                )
                | (
                    410,
                    Some("invite_expired" | "invite_used" | "invite_revoked")
                )
                | (429, Some("quota"))
        );
        return Err(if definitive {
            SendError::Refused(error)
        } else {
            SendError::Unknown(error)
        });
    }
    serde_json::from_str(&body)
        .context("membership reply is not JSON")
        .map_err(SendError::Unknown)
}

/// The key of a key file for `realm` at `issuer` and `provider` that an
/// interrupted `join` left at `path`, if any.
fn interrupted_join_key(
    path: &std::path::Path,
    realm: &str,
    issuer: &str,
    provider: &str,
) -> Result<Option<SigningKey>> {
    let ours = |file: &RealmClientKeyFile| {
        file.realm == realm && file.issuer == issuer && file.provider_id == provider
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    if !RealmClientKeyFile::parse(&text).is_ok_and(|(file, _)| ours(&file)) {
        return Ok(None);
    }
    // Reused only if it is still readable by its owner alone.
    let (file, key) = RealmClientKeyFile::read(path)?;
    Ok(ours(&file).then_some(key))
}

/// Redeem the invite. The key file is written before the request is sent,
/// so a lost reply does not lose the key of a redeemed invite: rerunning the
/// same command reuses the key file, and the Actr Facility Provider answers the
/// retry of that redemption with its result (or `conflict` if the key is
/// already registered otherwise).
pub async fn join(args: &JoinArgs) -> Result<Value> {
    let client = discovery_client()?;
    join_with(
        args,
        move |url| {
            let client = client.clone();
            async move { fetch_public(&client, url).await }
        },
        &http_client()?,
    )
    .await
}

/// [`join`], fetching public documents with `fetch` and sending membership
/// requests with `client`.
async fn join_with<F, Fut>(args: &JoinArgs, fetch: F, client: &reqwest::Client) -> Result<Value>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let text = if args.invite == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        args.invite.clone()
    };
    let invite = RealmInvite::decode(&text)?;
    // The invite secret goes only to an Issuer the domain's keys vouch for,
    // or to the one given on the command line.
    let (issuer, provider) = match &args.issuer {
        Some(issuer) => {
            let provider = match &args.provider {
                Some(provider) => provider.clone(),
                None => url_authority(issuer)?,
            };
            (issuer.clone(), provider)
        }
        None => {
            let profile = realm_profile(&invite.realm, invite.profile.as_ref(), fetch).await?;
            (profile.issuer.url, profile.provider.id)
        }
    };

    let reused = if args.output.exists() && !args.force {
        let key = interrupted_join_key(&args.output, &invite.realm, &issuer, &provider)?;
        if key.is_none() {
            bail!(
                "Key file already exists: {}\nUse --force to overwrite.",
                args.output.display()
            );
        }
        key
    } else {
        None
    };
    let created = reused.is_none();
    let key = match reused {
        Some(key) => key,
        None => {
            let key = SigningKey::from_bytes(&rand::random::<[u8; 32]>());
            if let Some(parent) = args.output.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create {}", parent.display()))?;
            }
            let file = RealmClientKeyFile::new(&invite.realm, &provider, &issuer, &key);
            write_private(
                &args.output,
                serde_json::to_string_pretty(&file)?.as_bytes(),
            )
            .with_context(|| format!("Failed to write key file: {}", args.output.display()))?;
            key
        }
    };

    let mut request =
        MembershipRequest::new(MembershipOp::Join, &invite.realm, &provider, &new_nonce());
    request.invite = Some(invite.secret.clone());
    request.public_key = Some(BASE64.encode(key.verifying_key().to_bytes()));
    request.label = args.label.clone();
    match send_with(client, &issuer, &request.sign(&key)).await {
        Ok(reply) => {
            eprintln!("Realm client key written to {}", args.output.display());
            Ok(reply)
        }
        Err(SendError::Refused(error)) => {
            if created {
                let _ = std::fs::remove_file(&args.output);
            }
            Err(error)
        }
        Err(SendError::Unknown(error)) => Err(error.context(format!(
            "The invite may have been redeemed; the key is kept in {}. \
             Rerun the same command to finish joining.",
            args.output.display()
        ))),
    }
}

pub async fn invite(args: &InviteArgs) -> Result<Value> {
    if args.days == 0 || args.days > 30 {
        bail!("--days must be 1 to 30");
    }
    let member = Member::load(&args.key)?;
    let mut request = member.request(MembershipOp::CreateInvite);
    request.role = args.role;
    request.member_id = args.member.clone();
    let expires = chrono::Utc::now() + chrono::Duration::days(args.days.into());
    request.invite_expires_at = Some(expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    member.send(request).await
}

pub async fn members(args: &KeyArgs) -> Result<Value> {
    let member = Member::load(args)?;
    member.send(member.request(MembershipOp::ListMembers)).await
}

pub async fn revoke_invite(args: &InviteIdArgs) -> Result<Value> {
    let member = Member::load(&args.key)?;
    let mut request = member.request(MembershipOp::RevokeInvite);
    request.invite_id = Some(args.invite_id.clone());
    member.send(request).await
}

pub async fn revoke_client(args: &RevokeClientArgs) -> Result<Value> {
    let member = Member::load(&args.key)?;
    let mut request = member.request(MembershipOp::RevokeClient);
    request.key_id = Some(args.key_id.clone().unwrap_or(member.file.key_id.clone()));
    member.send(request).await
}

pub async fn remove_member(args: &MemberArgs) -> Result<Value> {
    let member = Member::load(&args.key)?;
    let mut request = member.request(MembershipOp::RemoveMember);
    request.member_id = Some(args.member_id.clone());
    member.send(request).await
}

pub async fn set_role(args: &SetRoleArgs) -> Result<Value> {
    let member = Member::load(&args.key)?;
    let mut request = member.request(MembershipOp::SetRole);
    request.member_id = Some(args.member_id.clone());
    request.role = Some(args.role);
    member.send(request).await
}

pub async fn transfer_owner(args: &TransferOwnerArgs) -> Result<Value> {
    let member = Member::load(&args.key)?;
    let mut request = member.request(MembershipOp::TransferOwner);
    request.member_id = Some(args.member_id.clone());
    request.previous_owner_role = Some(args.previous_owner_role);
    member.send(request).await
}

pub async fn leave(args: &KeyArgs) -> Result<Value> {
    let member = Member::load(args)?;
    member.send(member.request(MembershipOp::Leave)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use actr_protocol::realm_profile::sign_document;
    use axum::{Json, Router, routing::post};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    const REALM: &str = "k7m2q9xh@acme.com";

    mod e2e_documents {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/e2e/actr/package-runtime-echo/fixtures/documents.rs"
        ));
    }

    /// Download the Profile-selected package before joining. This remains a
    /// test orchestrator, not automatic Host resolution (#324/#336).
    async fn download_realm_package(profile: &RealmProfile, output: &str) -> Result<()> {
        use sha2::{Digest, Sha256};
        use std::io::Write as _;

        let package = profile.package.as_ref().context("active Realm package")?;
        let bytes = e2e_documents::fetch_https(&package.href, 64 * 1024 * 1024).await?;
        anyhow::ensure!(
            hex::encode(Sha256::digest(&bytes)) == package.sha256,
            "PACKAGE_DIGEST_MISMATCH"
        );
        let manifest = actr_pack::read_runtime_manifest(&bytes)?;
        let identity = actr_pack::ValidatedActorIdentity::from_actor_type(manifest.actor_type())?;
        let publisher = ActrUri::from_domain(identity.publisher())?;
        let document =
            e2e_documents::fetch(publisher.discovery_url().context("Publisher URL")?).await?;
        let value: Value = serde_json::from_slice(&document)?;
        let descriptor: AuthorityDescriptor = serde_json::from_value(value.clone())?;
        descriptor.validate_for(&publisher)?;
        let profile: actr_protocol::PublisherProfile =
            serde_json::from_value(value["publisher"].clone())?;
        let keys = profile.verify(identity.publisher(), chrono::Utc::now())?;
        let key = keys
            .keys
            .iter()
            .find(|key| Some(key.id.as_str()) == manifest.signing_key_id())
            .context("package signing key is not listed by Publisher")?;
        let public_key: [u8; 32] = BASE64
            .decode(&key.public_key)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid package public key"))?;
        let verified = actr_pack::verify_runtime_package_bounded(
            &bytes,
            &ed25519_dalek::VerifyingKey::from_bytes(&public_key)?,
            actr_pack::DEFAULT_MAX_VERIFIED_ENTRY_BYTES,
        )?;
        anyhow::ensure!(
            verified.manifest.actor_type().to_program_identity() == package.r#type,
            "SERVICE_ACTOR_TYPE_MISMATCH"
        );
        let output = std::path::Path::new(output);
        let parent = output.parent().context("package output directory")?;
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.persist(output)?;
        Ok(())
    }

    /// Drives the real Profile resolver and join handler against the running
    /// E2E Issuer. HTTPS mode also downloads and verifies the selected package.
    #[tokio::test]
    #[ignore = "started by package-runtime-echo/lib/realm-scenarios.sh"]
    async fn realm_runtime_join_fixture() -> Result<()> {
        let request: Value =
            serde_json::from_slice(&std::fs::read(std::env::var("ACTR_REALM_E2E_REQUEST")?)?)?;
        let invite_path = request["invite_file"].as_str().context("invite_file")?;
        let invite_text = std::fs::read_to_string(invite_path)?;
        let invite = RealmInvite::decode(&invite_text)?;
        let profile =
            realm_profile(&invite.realm, invite.profile.as_ref(), e2e_documents::fetch).await?;
        if let Some(output) = request["package_file"].as_str() {
            download_realm_package(&profile, output).await?;
        }
        let args = JoinArgs {
            invite: invite_text,
            output: request["key_file"].as_str().context("key_file")?.into(),
            issuer: None,
            provider: None,
            label: Some(request["label"].as_str().context("label")?.into()),
            force: false,
        };
        let joined = join_with(&args, e2e_documents::fetch, &http_client()?).await?;
        std::fs::write(
            request["result_file"].as_str().context("result_file")?,
            serde_json::to_vec(&serde_json::json!({ "joined": joined, "profile": profile }))?,
        )?;
        Ok(())
    }

    /// A membership endpoint that verifies `join` with its own key and any
    /// other request with the key that joined, and echoes the request.
    async fn serve() -> String {
        let joined = std::sync::Arc::new(std::sync::Mutex::new(None::<[u8; 32]>));
        let app = Router::new().route(
            "/issuer/membership",
            post(move |body: axum::body::Bytes| {
                let joined = joined.clone();
                async move {
                    let request = MembershipRequest::parse(&body).unwrap();
                    if request.op == MembershipOp::Join {
                        *joined.lock().unwrap() = Some(request.verify_join().unwrap());
                    } else {
                        request.verify(&joined.lock().unwrap().unwrap()).unwrap();
                    }
                    Json(serde_json::from_slice::<Value>(&body).unwrap())
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}/issuer")
    }

    #[tokio::test]
    async fn join_writes_a_key_file_the_other_commands_sign_with() {
        let issuer = serve().await;
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("keys/client.json");
        let secret = URL_SAFE_NO_PAD.encode([1u8; 32]);
        let join_args = JoinArgs {
            invite: RealmInvite::new(REALM, &secret).encode(),
            output: output.clone(),
            issuer: Some(issuer.clone()),
            provider: None,
            label: Some("laptop".into()),
            force: false,
        };
        let sent = join(&join_args).await.unwrap();
        assert_eq!(sent["op"], "join");
        assert_eq!(sent["invite"], secret);
        assert_eq!(sent["label"], "laptop");
        // No Realm Profile: the provider is the Issuer URL's authority.
        assert_eq!(sent["provider_id"], url_authority(&issuer).unwrap());
        let key = KeyArgs {
            key: output.clone(),
        };
        assert_eq!(Member::load(&key).unwrap().file.realm, REALM);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&output).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A rerun reuses the key file (retry of an interrupted join); a key
        // file of another Realm is not overwritten by accident.
        let file = Member::load(&key).unwrap().file;
        assert_eq!(
            join(&join_args).await.unwrap()["public_key"],
            file.public_key
        );
        assert!(
            join(&JoinArgs {
                invite: RealmInvite::new("other@acme.com", &secret).encode(),
                ..join_args.clone()
            })
            .await
            .is_err()
        );
        assert_eq!(Member::load(&key).unwrap().file.public_key, file.public_key);

        let sent = invite(&InviteArgs {
            key: KeyArgs {
                key: key.key.clone(),
            },
            role: Some(Role::Editor),
            member: None,
            days: 7,
        })
        .await
        .unwrap();
        assert_eq!(
            (sent["op"].as_str(), sent["role"].as_str()),
            (Some("create_invite"), Some("editor"))
        );

        let sent = revoke_client(&RevokeClientArgs {
            key: KeyArgs {
                key: key.key.clone(),
            },
            key_id: None,
        })
        .await
        .unwrap();
        let file = Member::load(&key).unwrap().file;
        assert_eq!(sent["key_id"], file.key_id.as_str());

        let sent = transfer_owner(&TransferOwnerArgs {
            key: KeyArgs {
                key: key.key.clone(),
            },
            member_id: "0123456789abcdef".into(),
            previous_owner_role: Role::Viewer,
        })
        .await
        .unwrap();
        assert_eq!(sent["previous_owner_role"], "viewer");
        assert_eq!(leave(&key).await.unwrap()["op"], "leave");
    }

    /// A membership endpoint that reads each request, sends it on the
    /// returned channel and closes the connection without a reply.
    async fn serve_without_reply() -> (String, tokio::sync::mpsc::UnboundedReceiver<Value>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let read = stream.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    let text = String::from_utf8_lossy(&buffer);
                    if let Some(start) = text.find("\r\n\r\n")
                        && let Ok(body) = serde_json::from_str::<Value>(&text[start + 4..])
                    {
                        let _ = tx.send(body);
                        break;
                    }
                }
                let _ = stream.shutdown().await;
            }
        });
        (format!("http://{address}/issuer"), rx)
    }

    #[tokio::test]
    async fn a_join_whose_reply_is_lost_keeps_its_key_for_the_retry() {
        let (lost, mut sent) = serve_without_reply().await;
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("client.json");
        let args = JoinArgs {
            invite: RealmInvite::new(REALM, &URL_SAFE_NO_PAD.encode([3u8; 32])).encode(),
            output: output.clone(),
            issuer: Some(lost),
            provider: Some("p.example".into()),
            label: None,
            force: false,
        };
        let error = join(&args).await.unwrap_err();
        assert!(format!("{error:#}").contains("Rerun the same command"));
        let first = sent.recv().await.unwrap();
        // The key the Actr Facility Provider may have registered is on disk.
        let file = Member::load(&KeyArgs {
            key: output.clone(),
        })
        .unwrap()
        .file;
        assert_eq!(first["public_key"], file.public_key);

        // Rerunning the command retries the same redemption with that key.
        assert!(join(&args).await.is_err());
        let retry = sent.recv().await.unwrap();
        assert_eq!(retry["public_key"], file.public_key);
        assert_eq!(retry["invite"], first["invite"]);

        // A rerun naming another Actr Facility Provider does not reuse it: the key
        // file would name one Actr Facility Provider and the join another.
        let other = JoinArgs {
            provider: Some("q.example".into()),
            ..args.clone()
        };
        let error = join(&other).await.unwrap_err();
        assert!(error.to_string().contains("already exists"), "{error:#}");
    }

    #[tokio::test]
    async fn committed_join_with_gateway_error_keeps_the_registered_key() {
        for status in [502, 504, 500, 408] {
            let registered = std::sync::Arc::new(std::sync::Mutex::new(None::<[u8; 32]>));
            let app = Router::new().route(
                "/issuer/membership",
                post({
                    let registered = registered.clone();
                    move |body: axum::body::Bytes| {
                        let registered = registered.clone();
                        async move {
                            let request = MembershipRequest::parse(&body).unwrap();
                            let public_key = request.verify_join().unwrap();
                            let mut stored = registered.lock().unwrap();
                            if let Some(original) = *stored {
                                assert_eq!(public_key, original, "retry must use the redeemed key");
                                (axum::http::StatusCode::OK, "{\"joined\":true}")
                            } else {
                                *stored = Some(public_key); // redemption committed
                                (
                                    axum::http::StatusCode::from_u16(status).unwrap(),
                                    "gateway response lost",
                                )
                            }
                        }
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let dir = tempfile::tempdir().unwrap();
            let args = JoinArgs {
                invite: RealmInvite::new(REALM, &URL_SAFE_NO_PAD.encode([9u8; 32])).encode(),
                output: dir.path().join("client.json"),
                issuer: Some(format!("http://{address}/issuer")),
                provider: None,
                label: None,
                force: false,
            };
            let error = join(&args).await.unwrap_err();
            assert!(format!("{error:#}").contains("invite may have been redeemed"));
            let before = std::fs::read(&args.output).unwrap();
            assert_eq!(join(&args).await.unwrap()["joined"], true);
            assert_eq!(std::fs::read(&args.output).unwrap(), before);
            server.abort();
        }
    }

    #[tokio::test]
    async fn a_refused_join_leaves_no_key_file() {
        let app = Router::new().route(
            "/issuer/membership",
            post(|| async {
                (
                    axum::http::StatusCode::GONE,
                    Json(serde_json::json!({"error": "invite_used", "message": "used"})),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("client.json");
        let error = join(&JoinArgs {
            invite: RealmInvite::new(REALM, &URL_SAFE_NO_PAD.encode([4u8; 32])).encode(),
            output: output.clone(),
            issuer: Some(format!("http://{address}/issuer")),
            provider: None,
            label: None,
            force: false,
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("invite_used"));
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn join_does_not_trust_an_embedded_realm_profile() {
        let issuer = serve().await;
        let dir = tempfile::tempdir().unwrap();
        let mut invite = RealmInvite::new(REALM, &URL_SAFE_NO_PAD.encode([2u8; 32]));
        // Unverified: its issuer and provider must not be used.
        invite.profile = Some(serde_json::json!({
            "realm": { "name": REALM },
            "provider": { "id": "evil.example" },
            "issuer": { "url": "http://127.0.0.1:9/issuer" },
        }));
        let sent = join(&JoinArgs {
            invite: invite.encode(),
            output: dir.path().join("client.json"),
            issuer: Some(issuer.clone()),
            provider: None,
            label: None,
            force: false,
        })
        .await
        .unwrap();
        assert_eq!(sent["provider_id"], url_authority(&issuer).unwrap());
    }

    /// A Realm signing key's self-signed `realm_signing_keys` member in an
    /// authority descriptor of `acme.com` whose `realms` link is
    /// `https://acme.com/actr/realms/`.
    fn descriptor(key: &SigningKey) -> Value {
        let mut member = serde_json::json!({
            "sequence": 1,
            "keys": [{
                "id": "rs-1", "algorithm": "Ed25519", "status": "active",
                "public_key": BASE64.encode(key.verifying_key().to_bytes()),
            }],
        });
        sign_document(&mut member, "rs-1", key);
        serde_json::json!({
            "version": 1,
            "authority": "actr://acme.com",
            "services": [{
                "rel": "realms",
                "href": "https://acme.com/actr/realms/",
                "protocol": "actr-realm-profile/1",
            }],
            "realm_signing_keys": member,
        })
    }

    /// A Realm Profile of `REALM` naming `issuer` and `provider`, signed by
    /// `key` as `rs-1`.
    fn profile(key: &SigningKey, sequence: u64, issuer: &str, provider: &str) -> Value {
        let mut profile = serde_json::json!({
            "schema": "actr.realm/v1",
            "sequence": sequence,
            "realm": { "name": REALM, "display_name": "Notes", "status": "active" },
            "package": {
                "href": "https://cdn.example/a.actr", "sha256": "ab".repeat(32),
                "type": "acme.com/notes@1.0.0",
            },
            "issuer": { "url": issuer, "keys": [{
                "id": "7", "algorithm": "Ed25519", "status": "active",
                "public_key": BASE64.encode([2u8; 32]),
            }] },
            "provider": { "id": provider, "signaling": [], "ice": [] },
            "policy": { "membership": "invite-only" },
            "issued_at": "2026-09-25T00:00:00Z",
        });
        sign_document(&mut profile, "rs-1", key);
        profile
    }

    /// A fixture of `https://acme.com` (descriptor of `key`, and the Realm
    /// Profile `served` makes from the base URL, if any) and of its Issuer's
    /// membership endpoint; returns the base URL and a client that trusts
    /// the fixture's disposable TLS certificate.
    async fn serve_domain(
        key: &SigningKey,
        served: impl FnOnce(&str) -> Option<Value>,
    ) -> (String, reqwest::Client) {
        use axum::routing::get;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("https://{}", listener.local_addr().unwrap());
        let descriptor = descriptor(key);
        let served = served(&base);
        let app =
            Router::new()
                .route(
                    "/.well-known/actr",
                    get(move || async move { Json(descriptor) }),
                )
                .route(
                    "/actr/realms/k7m2q9xh",
                    get(move || async move {
                        served.map(Json).ok_or(axum::http::StatusCode::NOT_FOUND)
                    }),
                )
                .route(
                    "/issuer/membership",
                    post(|body: axum::body::Bytes| async move {
                        MembershipRequest::parse(&body)
                            .unwrap()
                            .verify_join()
                            .unwrap();
                        Json(serde_json::from_slice::<Value>(&body).unwrap())
                    }),
                );
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let tls = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
        let client = reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_der(tls.cert.der()).unwrap())
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let config = axum_server::tls_rustls::RustlsConfig::from_pem(
            tls.cert.pem().into_bytes(),
            tls.signing_key.serialize_pem().into_bytes(),
        )
        .await
        .unwrap();
        let listener = listener.into_std().unwrap();
        tokio::spawn(async move {
            axum_server::from_tcp_rustls(listener, config)
                .unwrap()
                .serve(app.into_make_service())
                .await
                .unwrap()
        });
        (base, client)
    }

    /// Fetches `https://acme.com/…` from the fixture at `base`.
    fn fetch_from(
        base: String,
        client: reqwest::Client,
    ) -> impl Fn(String) -> futures_util::future::BoxFuture<'static, Result<Vec<u8>>> {
        move |url: String| {
            let url = url.replace("https://acme.com", &base);
            let client = client.clone();
            Box::pin(async move {
                let reply = client.get(&url).send().await?.error_for_status()?;
                Ok(reply.bytes().await?.to_vec())
            })
        }
    }

    fn join_args(invite: RealmInvite, output: PathBuf) -> JoinArgs {
        JoinArgs {
            invite: invite.encode(),
            output,
            issuer: None,
            provider: None,
            label: None,
            force: false,
        }
    }

    #[tokio::test]
    async fn join_takes_the_issuer_from_the_verified_realm_profile() {
        let key = SigningKey::from_bytes(&[8; 32]);
        let (base, client) = serve_domain(&key, |base| {
            Some(profile(&key, 3, &format!("{base}/issuer"), "p.example"))
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("client.json");
        let secret = URL_SAFE_NO_PAD.encode([5u8; 32]);

        let sent = join_with(
            &join_args(RealmInvite::new(REALM, &secret), output.clone()),
            fetch_from(base.clone(), client.clone()),
            &client,
        )
        .await
        .unwrap();
        assert_eq!(
            (sent["op"].as_str(), sent["provider_id"].as_str()),
            (Some("join"), Some("p.example"))
        );
        let file = Member::load(&KeyArgs { key: output }).unwrap().file;
        assert_eq!(file.issuer, format!("{base}/issuer"));
        assert_eq!(file.provider_id, "p.example");
    }

    #[tokio::test]
    async fn join_uses_an_embedded_realm_profile_only_if_the_domain_keys_verify_it() {
        let key = SigningKey::from_bytes(&[8; 32]);
        let forger = SigningKey::from_bytes(&[9; 32]);
        // The domain serves no Realm Profile at its realms location.
        let (base, client) = serve_domain(&key, |_| None).await;
        let issuer = format!("{base}/issuer");
        let dir = tempfile::tempdir().unwrap();
        let secret = URL_SAFE_NO_PAD.encode([6u8; 32]);

        // Signed by a key the domain does not list: refused, nothing sent.
        let mut invite = RealmInvite::new(REALM, &secret);
        invite.profile = Some(profile(&forger, 9, &issuer, "evil.example"));
        let output = dir.path().join("forged.json");
        let error = join_with(
            &join_args(invite, output.clone()),
            fetch_from(base.clone(), client.clone()),
            &client,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("No verified Realm Profile"),
            "{error:#}"
        );
        assert!(!output.exists());

        // Signed by the domain's key: used.
        let mut invite = RealmInvite::new(REALM, &secret);
        invite.profile = Some(profile(&key, 2, &issuer, "p.example"));
        let output = dir.path().join("embedded.json");
        let sent = join_with(
            &join_args(invite, output),
            fetch_from(base, client.clone()),
            &client,
        )
        .await
        .unwrap();
        assert_eq!(sent["provider_id"], "p.example");
    }

    #[tokio::test]
    async fn join_prefers_the_newer_of_the_fetched_and_embedded_profiles() {
        let key = SigningKey::from_bytes(&[8; 32]);
        let (base, client) = serve_domain(&key, |base| {
            Some(profile(&key, 4, &format!("{base}/issuer"), "p.example"))
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let secret = URL_SAFE_NO_PAD.encode([7u8; 32]);

        // An older embedded Profile loses to the fetched one.
        let mut invite = RealmInvite::new(REALM, &secret);
        invite.profile = Some(profile(&key, 1, "https://127.0.0.1:9/old", "p.example"));
        let output = dir.path().join("a.json");
        join_with(
            &join_args(invite, output.clone()),
            fetch_from(base.clone(), client.clone()),
            &client,
        )
        .await
        .unwrap();
        let file = Member::load(&KeyArgs { key: output }).unwrap().file;
        assert_eq!(file.issuer, format!("{base}/issuer"));

        // A newer one naming another Actr Facility Provider is not a valid successor.
        let mut invite = RealmInvite::new(REALM, &secret);
        invite.profile = Some(profile(&key, 5, &format!("{base}/issuer"), "q.example"));
        let error = join_with(
            &join_args(invite, dir.path().join("b.json")),
            fetch_from(base, client.clone()),
            &client,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("provider"), "{error}");
    }

    #[tokio::test]
    async fn join_without_a_realm_profile_needs_the_issuer() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("client.json");
        let error = join_with(
            &join_args(
                RealmInvite::new(REALM, &URL_SAFE_NO_PAD.encode([1u8; 32])),
                output.clone(),
            ),
            |url| async move { Err(anyhow::anyhow!("{url}: 404 Not Found")) },
            &http_client().unwrap(),
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("--issuer"), "{error:#}");
        assert!(!output.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn key_files_others_can_read_are_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let issuer = serve().await;
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("client.json");
        let mut args = join_args(
            RealmInvite::new(REALM, &URL_SAFE_NO_PAD.encode([1u8; 32])),
            output.clone(),
        );
        args.issuer = Some(issuer);
        join(&args).await.unwrap();

        std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o644)).unwrap();
        let key = KeyArgs { key: output };
        let error = members(&key).await.unwrap_err();
        assert!(format!("{error:#}").contains("chmod 600"), "{error:#}");
        // Nor is it reused by a rerun of the join.
        let error = join(&args).await.unwrap_err();
        assert!(format!("{error:#}").contains("chmod 600"), "{error:#}");
    }

    #[tokio::test]
    async fn public_documents_are_limited_to_64_kib() {
        use axum::body::Body;
        use axum::routing::get;
        let big = || vec![b' '; 64 * 1024 + 1];
        let app = Router::new()
            .route("/fits", get(|| async { vec![b' '; 64 * 1024] }))
            .route("/sized", get(move || async move { big() }))
            .route(
                "/chunked",
                get(|| async {
                    let chunks = (0..65).map(|_| Ok::<_, std::io::Error>(vec![b' '; 1024]));
                    Body::from_stream(futures_util::stream::iter(chunks))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = reqwest::Client::new();
        assert_eq!(
            fetch_public(&client, format!("{base}/fits"))
                .await
                .unwrap()
                .len(),
            64 * 1024
        );
        for path in ["sized", "chunked"] {
            let error = fetch_public(&client, format!("{base}/{path}"))
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains("64 KiB"), "{path}: {error:#}");
        }
    }

    #[test]
    fn roles_exclude_owner() {
        assert!(parse_role("owner").is_err());
        assert_eq!(parse_role("viewer").unwrap(), Role::Viewer);
        assert_eq!(
            url_authority("HTTP://Example.COM:8081/issuer").unwrap(),
            "example.com:8081"
        );
        assert_eq!(
            url_authority("https://example.com/issuer").unwrap(),
            "example.com"
        );
    }
}
