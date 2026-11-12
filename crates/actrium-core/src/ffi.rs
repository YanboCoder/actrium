use actr_protocol::authority::{
    self, AuthorityDescriptor, AuthorityDescriptorError, RealmSigningKeys,
};
use actr_protocol::realm_membership::RealmInvite;
use actr_protocol::realm_profile::{RealmProfile, RealmProfileError, RealmStatus};
use actr_protocol::uri::{ActrUri, ActrUriError};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::actr_package::{
    self, ActrPackageManifest, ContractGateStatus as DomainContractGateStatus, ManifestError,
};
use crate::auth;
use crate::bridge;
use crate::browser;
use crate::browser_data;
use crate::domain::{AppSourceType, Contract, SystemAppType, Tag as DomainTag};
use crate::persistence;
use crate::realm_link;
use crate::runtime_persistence;
use crate::speech;

#[derive(Debug, Clone)]
pub struct ActrPackageManifestInfo {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub app_id: Option<String>,
    pub icon: Option<String>,
    pub entry: String,
    pub left_panel_default_visible: bool,
    pub right_panel_default_visible: bool,
    pub contract_name: Option<String>,
    pub contract_version: Option<u32>,
    pub permissions: Vec<String>,
    pub system_app_type: Option<String>,
    pub theme_color: Option<String>,
}

impl From<ActrPackageManifest> for ActrPackageManifestInfo {
    fn from(manifest: ActrPackageManifest) -> Self {
        let extension = manifest.extensions.actrium.as_ref();
        let is_keel_guest = manifest.workload.profile == actr_pack::KEEL_GUEST_PROFILE_V1;

        Self {
            name: extension
                .and_then(|value| value.title.clone())
                .unwrap_or_else(|| manifest.actor.name.clone()),
            version: manifest.actor.version.clone(),
            description: extension.and_then(|value| value.description.clone()),
            app_id: Some(format!(
                "{}/{}@{}",
                manifest.actor.publisher, manifest.actor.name, manifest.actor.version
            )),
            icon: extension.and_then(|value| value.icon.clone()),
            entry: manifest.workload.entry,
            left_panel_default_visible: extension
                .is_some_and(|value| value.left_panel_default_visible),
            right_panel_default_visible: extension
                .is_some_and(|value| value.right_panel_default_visible),
            contract_name: is_keel_guest.then(|| "keel.guest".to_string()),
            contract_version: is_keel_guest.then_some(1),
            permissions: manifest.host.required_capabilities,
            system_app_type: None,
            theme_color: extension.and_then(|value| value.theme_color.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractGateStatus {
    Ok,
    Compat,
    Blocked,
}

impl From<DomainContractGateStatus> for ContractGateStatus {
    fn from(status: DomainContractGateStatus) -> Self {
        match status {
            DomainContractGateStatus::Ok => Self::Ok,
            DomainContractGateStatus::Compat => Self::Compat,
            DomainContractGateStatus::Blocked => Self::Blocked,
        }
    }
}

impl From<browser::BrowserSearchEngine> for BrowserSearchEngineInfo {
    fn from(engine: browser::BrowserSearchEngine) -> Self {
        Self {
            id: engine.id.to_string(),
            display_name: engine.display_name.to_string(),
            search_url_prefix: engine.search_url_prefix.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContractGateInfo {
    pub status: ContractGateStatus,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SystemAppInfo {
    pub kind: String,
    pub title: String,
    pub about_url: String,
}

#[derive(Debug, Clone)]
pub struct AppSourceTypeInfo {
    pub kind: String,
    pub display_name: String,
}

#[derive(Debug, Clone)]
pub struct BrowserSearchEngineInfo {
    pub id: String,
    pub display_name: String,
    pub search_url_prefix: String,
}

#[derive(Debug, Clone)]
pub struct BrowserDataContractInfo {
    pub schema_version: u32,
    pub max_history_items: u32,
    pub max_bookmark_items: u32,
    pub max_session_tabs: u32,
    pub max_title_length: u32,
    pub home_address: String,
    pub history_address: String,
    pub bookmarks_address: String,
}

#[derive(Debug, Clone)]
pub struct BrowserStorageSchemaInfo {
    pub schema_version: u32,
    pub database_name: String,
    pub table_names: Vec<String>,
    pub statements: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BrowserStorageRepositorySqlInfo {
    pub upsert_history_page: String,
    pub insert_history_visit: String,
    pub select_history_recent: String,
    pub delete_history_by_address: String,
    pub delete_all_history: String,
    pub upsert_bookmark: String,
    pub select_bookmarks_recent: String,
    pub delete_bookmark_by_address: String,
    pub delete_bookmark_by_id: String,
    pub delete_all_bookmarks: String,
    pub upsert_session: String,
    pub select_session: String,
    pub delete_session_tabs: String,
    pub insert_session_tab: String,
    pub select_session_tabs: String,
}

#[derive(Debug, Clone)]
pub struct BrowserStorageSyncSqlInfo {
    pub select_dirty_history_pages: String,
    pub select_dirty_history_visits: String,
    pub select_dirty_bookmark_folders: String,
    pub select_dirty_bookmarks: String,
    pub select_dirty_sessions: String,
    pub select_dirty_session_tabs: String,
    pub apply_remote_history_page: String,
    pub apply_remote_history_visit: String,
    pub apply_remote_bookmark_folder: String,
    pub apply_remote_bookmark: String,
    pub apply_remote_session: String,
    pub apply_remote_session_tab: String,
    pub mark_history_page_synced: String,
    pub mark_history_visit_synced: String,
    pub mark_bookmark_folder_synced: String,
    pub mark_bookmark_synced: String,
    pub mark_session_synced: String,
    pub mark_session_tab_synced: String,
    pub upsert_sync_cursor: String,
    pub select_sync_cursors_for_scope: String,
}

#[derive(Debug, Clone)]
pub struct DefaultTagInfo {
    pub name: String,
    pub color: String,
    pub is_system: bool,
}

#[derive(Debug, Clone)]
pub struct AsrSegmentInfo {
    pub word: String,
    pub nature: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AsrModelPlanInfo {
    pub language_dir: String,
    pub asset_path: String,
    pub storage_relative_path: String,
    pub ready_marker: String,
}

#[derive(Debug, Clone)]
pub struct AsrBridgeContractInfo {
    pub module_name: String,
    pub result_event_name: String,
    pub error_event_name: String,
    pub error_init: String,
    pub error_feed: String,
    pub error_stop: String,
    pub key_text: String,
    pub key_is_final: String,
}

#[derive(Debug, Clone)]
pub struct TtsBridgeContractInfo {
    pub module_name: String,
    pub event_start: String,
    pub event_pause: String,
    pub event_resume: String,
    pub event_finish: String,
    pub event_error: String,
    pub key_rate: String,
    pub key_pitch_multiplier: String,
    pub key_volume: String,
    pub key_language: String,
    pub key_voice_identifier: String,
    pub key_cancelled: String,
    pub key_message: String,
}

#[derive(Debug, Clone)]
pub struct EmailBridgeContractInfo {
    pub module_name: String,
    pub method_compose_email: String,
    pub uri_scheme_mailto: String,
    pub error_invalid_recipients: String,
    pub error_open_failed: String,
}

#[derive(Debug, Clone)]
pub struct UserInfoBridgeContractInfo {
    pub module_name: String,
    pub method_get_current_user_info: String,
    pub method_login_with_type: String,
    pub event_user_info_changed: String,
    pub event_login_status_changed: String,
    pub event_user_logout: String,
    pub key_id: String,
    pub key_login_api_url: String,
    pub key_user_info: String,
    pub key_user_identity: String,
    pub key_session_token: String,
    pub key_created_at: String,
    pub key_updated_at: String,
    pub key_request_context: String,
    pub key_is_logged_in: String,
}

#[derive(Debug, Clone)]
pub struct ActrBridgeContractInfo {
    pub module_name: String,
    pub stream_event_name: String,
    pub status_connected: String,
    pub stream_type_text: String,
    pub stream_type_voice: String,
    pub stream_type_attachment_result: String,
    pub stream_status_ok: i32,
    pub stream_status_error: i32,
    pub stream_metadata_eos_key: String,
    pub stream_metadata_aos_key: String,
    pub stream_metadata_terminal_value: String,
    pub attachment_result_timeout_ms: u32,
    pub attachment_chunk_size_bytes: u32,
    pub attachment_checksum_algorithm_md5: String,
    pub attachment_status_code_ok: i32,
    pub attachment_type_unknown: i32,
    pub attachment_type_image: i32,
    pub attachment_type_document: i32,
    pub attachment_type_audio: i32,
    pub attachment_type_video: i32,
    pub attachment_type_other: i32,
    pub attachment_upload_status_unknown: i32,
    pub attachment_upload_status_completed: i32,
    pub attachment_upload_status_failed: i32,
    pub ask_service_prompt_route_key: String,
    pub ask_service_attach_route_key: String,
    pub ask_service_prepare_attachment_upload_route_key: String,
    pub ask_local_service_send_text_route_key: String,
    pub ask_local_service_start_send_voice_route_key: String,
    pub ask_local_service_send_voice_chunk_route_key: String,
    pub ios_ask_service_prompt_timeout_ms: u32,
    pub ios_ask_service_prepare_attachment_upload_timeout_ms: u32,
    pub android_ask_service_prompt_timeout_ms: u32,
    pub android_ask_service_attach_timeout_ms: u32,
    pub android_ask_local_service_rpc_timeout_ms: u32,
    pub error_invalid_input: String,
    pub error_unknown: String,
    pub error_prompt_request: String,
    pub error_not_connected: String,
    pub error_actr: String,
    pub error_generic: String,
    pub key_tab_id: String,
    pub key_actor_type: String,
    pub key_path: String,
    pub key_status: String,
    pub key_message_id: String,
    pub key_session_id: String,
    pub key_text: String,
    pub key_message: String,
    pub key_stream_id: String,
    pub key_stream_id_upper: String,
    pub key_text_response_stream_id: String,
    pub key_voice_response_stream_id: String,
    pub key_attachment_ids: String,
    pub key_location: String,
    pub key_latitude: String,
    pub key_longitude: String,
    pub key_address: String,
    pub key_place_name: String,
    pub key_file_data: String,
    pub key_attachment_id: String,
    pub key_checksum: String,
    pub key_checksum_algorithm: String,
    pub key_size_bytes: String,
    pub key_result_stream_id: String,
    pub key_id: String,
    pub key_filename: String,
    pub key_type: String,
    pub key_status_code: String,
    pub key_error_message: String,
    pub key_question_id: String,
    pub key_chunk_id: String,
    pub key_metadata: String,
    pub key_metadata_key: String,
    pub key_metadata_value: String,
    pub key_sequence: String,
    pub key_timestamp_ms: String,
}

#[derive(Debug, Clone)]
pub struct ActrActorTypeInfo {
    pub publisher: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct CoreSqlValueInfo {
    pub kind: String,
    pub text: Option<String>,
    pub integer: Option<i64>,
    pub real: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct CoreHistoryRepositorySqlInfo {
    pub insert_visit: String,
    pub select_recent: String,
    pub select_all: String,
    pub search_by_title: String,
    pub delete_by_id: String,
    pub delete_all: String,
}

#[derive(Debug, Clone)]
pub struct CoreHistoryItemRowInfo {
    pub id: String,
    pub title: String,
    pub app_url: Option<String>,
    pub app_id: Option<String>,
    pub visit_time_seconds: i64,
}

#[derive(Debug, Clone)]
pub struct CoreHistoryItemInfo {
    pub id: String,
    pub title: String,
    pub app_url: Option<String>,
    pub app_id: Option<String>,
    pub visit_time_seconds: i64,
}

#[derive(Debug, Clone)]
pub struct CoreTagRepositorySqlInfo {
    pub count_all: String,
    pub select_all: String,
    pub select_user: String,
    pub insert_tag: String,
    pub update_tag: String,
    pub delete_user_tag_by_id: String,
    pub select_by_bookmark_id: String,
    pub delete_bookmark_tags_by_bookmark_id: String,
    pub insert_bookmark_tag: String,
    pub insert_or_ignore_bookmark_tag: String,
    pub delete_bookmark_tag: String,
}

#[derive(Debug, Clone)]
pub struct CoreTagRowInfo {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub is_system_value: i64,
}

#[derive(Debug, Clone)]
pub struct CoreTagInfo {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub is_system: bool,
}

#[derive(Debug, Clone)]
pub struct CoreBookmarkRepositorySqlInfo {
    pub insert_bookmark: String,
    pub update_bookmark: String,
    pub delete_by_id: String,
    pub select_all: String,
    pub select_by_tag: String,
    pub search_by_title_or_notes: String,
    pub count_by_app_url: String,
    pub select_by_app_url: String,
    pub update_last_used: String,
    pub delete_null_app_url: String,
    pub delete_duplicates_by_app_url: String,
    pub delete_bookmark_tags: String,
    pub delete_all: String,
    pub vacuum: String,
}

#[derive(Debug, Clone)]
pub struct CoreBookmarkRowInfo {
    pub id: String,
    pub title: String,
    pub app_url: Option<String>,
    pub app_id: Option<String>,
    pub created_at_seconds: i64,
    pub last_used_at_seconds: Option<i64>,
    pub publisher: Option<String>,
    pub notes: Option<String>,
    pub favicon: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CoreBookmarkInfo {
    pub id: String,
    pub title: String,
    pub app_url: Option<String>,
    pub app_id: Option<String>,
    pub created_at_seconds: i64,
    pub last_used_at_seconds: Option<i64>,
    pub publisher: Option<String>,
    pub notes: Option<String>,
    pub favicon: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CoreAppRepositorySqlInfo {
    pub insert_or_replace_app: String,
    pub update_app: String,
    pub delete_by_id: String,
    pub delete_by_bundle_id: String,
    pub select_all: String,
    pub select_by_source_type: String,
    pub select_pinned: String,
    pub select_by_bundle_id: String,
    pub select_by_id: String,
    pub count_by_bundle_id: String,
    pub update_last_used_by_bundle_id: String,
    pub toggle_pinned_by_id: String,
    pub search_by_name_description_or_bundle_id: String,
    pub delete_all: String,
    pub vacuum: String,
}

#[derive(Debug, Clone)]
pub struct CoreInstalledAppRowInfo {
    pub id: Option<String>,
    pub name: Option<String>,
    pub bundle_id: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub source_type: Option<String>,
    pub source_path: Option<String>,
    pub icon: Option<String>,
    pub installed_at_seconds: Option<i64>,
    pub last_used_at_seconds: Option<i64>,
    pub is_pinned_value: i64,
}

#[derive(Debug, Clone)]
pub struct CoreInstalledAppInfo {
    pub id: String,
    pub name: String,
    pub bundle_id: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub source_type: String,
    pub source_path: String,
    pub icon: Option<String>,
    pub installed_at_seconds: i64,
    pub last_used_at_seconds: Option<i64>,
    pub is_pinned: bool,
}

#[derive(Debug, Clone)]
pub struct PackageIntegrityFilePlanInfo {
    pub path: String,
    pub expected_hash: String,
}

impl From<actr_package::ManifestIntegrityFilePlan> for PackageIntegrityFilePlanInfo {
    fn from(plan: actr_package::ManifestIntegrityFilePlan) -> Self {
        Self {
            path: plan.path,
            expected_hash: plan.expected_hash,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PackageIntegrityPlanInfo {
    pub algorithm: String,
    pub files: Vec<PackageIntegrityFilePlanInfo>,
}

impl From<actr_package::ManifestIntegrityPlan> for PackageIntegrityPlanInfo {
    fn from(plan: actr_package::ManifestIntegrityPlan) -> Self {
        Self {
            algorithm: plan.algorithm,
            files: plan
                .files
                .into_iter()
                .map(PackageIntegrityFilePlanInfo::from)
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActiveTabInfo {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub screenshot: String,
    pub has_unread: bool,
    pub url: String,
    pub is_marked: bool,
    pub panel_visible: bool,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct ActiveTabRowInfo {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub screenshot: String,
    pub has_unread: bool,
    pub url: String,
    pub is_marked: bool,
    pub status: String,
    pub sort_order: i32,
}

#[derive(Debug, Clone)]
pub struct RuntimeTabUpdateInfo {
    pub tab_id: String,
    pub title: Option<String>,
    pub icon: Option<String>,
    pub app_url: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeTabUnreadInfo {
    pub tab_id: String,
    pub unread: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeTabBookmarkStatusInfo {
    pub tab_id: String,
    pub marked: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeTabBookmarkEventInfo {
    pub tab_id: String,
    pub is_bookmarked: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeTabFullscreenEventInfo {
    pub tab_id: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeExtensionPanelVisibleEventInfo {
    pub tab_id: String,
    pub visible: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeContainerSizeChangeEventInfo {
    pub container_width: i32,
    pub container_height: i32,
}

#[derive(Debug, Clone)]
pub struct RuntimeTabInfo {
    pub unified_script: String,
    pub right_panel_default_visible: bool,
    pub id: String,
    pub background_color: String,
    pub reload_key: i32,
    pub html_content: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeRootPropsInfo {
    pub tab: RuntimeTabInfo,
    pub is_left_panel_visible: bool,
    pub tabs: Vec<RuntimeTabInfo>,
    pub single_tab_mode: bool,
    pub is_right_panel_visible: bool,
    pub current_tab_id: String,
    pub container_height: i32,
    pub container_width: i32,
    pub app_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeSystemTabInfo {
    pub system_app_type: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeSystemRootPropsInfo {
    pub tab: RuntimeSystemTabInfo,
    pub single_tab_mode: bool,
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("manifest is invalid: {0}")]
    InvalidJson(String),
    #[error("manifest is missing required field `{0}`")]
    MissingField(String),
    #[error("unsupported contract `{actual}`, expected `{expected}`")]
    UnsupportedContract { actual: String, expected: String },
    #[error("contract `{name}` version {actual} is newer than supported version {supported}")]
    ContractTooNew {
        name: String,
        actual: u32,
        supported: u32,
    },
    #[error("unknown system app type `{0}`")]
    UnknownSystemAppType(String),
    #[error("unknown app source type `{0}`")]
    UnknownAppSourceType(String),
    #[error("invalid Actr actor type `{0}`, expected `publisher/program@version`")]
    InvalidActrActorType(String),
    #[error("invalid actr URI scheme")]
    UriSchemeInvalid,
    #[error("invalid actr URI Realm domain")]
    UriDomainInvalid,
    #[error("invalid actr URI RealmId or ActorId")]
    UriLabelInvalid,
    #[error("legacy actr URI form is not supported")]
    UriLegacyForm,
    #[error("local actr Realm does not belong to this installation")]
    UriLocalRealmForeign,
    #[error("invalid installation id, expected 26 lowercase base32 characters")]
    InvalidInstallationId,
    #[error("authority descriptor is invalid: {0}")]
    AuthorityDescriptorInvalid(String),
    #[error("Realm Profile is invalid: {0}")]
    RealmProfileInvalid(String),
    #[error("Realm '{0}' is retired")]
    RealmRetired(String),
}

impl From<ManifestError> for CoreError {
    fn from(error: ManifestError) -> Self {
        match error {
            ManifestError::InvalidManifest(message) => Self::InvalidJson(message),
            ManifestError::UnsupportedContract { actual, expected } => {
                Self::UnsupportedContract { actual, expected }
            }
            ManifestError::ContractTooNew {
                name,
                actual,
                supported,
            } => Self::ContractTooNew {
                name,
                actual,
                supported,
            },
        }
    }
}

pub fn parse_actr_package_manifest(source: String) -> Result<ActrPackageManifestInfo, CoreError> {
    actr_package::parse_manifest_toml(&source)
        .map(ActrPackageManifestInfo::from)
        .map_err(CoreError::from)
}

impl From<ActrUriError> for CoreError {
    fn from(error: ActrUriError) -> Self {
        // UDL error enums cannot carry fields, so the parse detail is dropped
        // here; the shell renders variant-specific copy and holds the raw
        // input for context.
        match error {
            ActrUriError::InvalidScheme(_) => Self::UriSchemeInvalid,
            ActrUriError::InvalidDomain(_) => Self::UriDomainInvalid,
            ActrUriError::InvalidLabel(_) | ActrUriError::InvalidPath(_) => Self::UriLabelInvalid,
            ActrUriError::LegacyForm(_) => Self::UriLegacyForm,
            ActrUriError::LocalRealmForeign(_) => Self::UriLocalRealmForeign,
            // `ActrUriError` is `#[non_exhaustive]`; future variants surface
            // as a label rejection until they get a dedicated mapping.
            _ => Self::UriLabelInvalid,
        }
    }
}

/// Parsed `actr://` Realm link or Actor address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActrUriInfo {
    /// Lowercase IDNA A-label domain, or `<installation-id>.actr.alt`.
    pub domain: String,
    pub realm_id: Option<String>,
    pub serial: Option<String>,
    /// Whether the link targets this installation's local Realm.
    pub is_local: bool,
    /// `https://<domain>/.well-known/actr`; `None` for a local Realm.
    pub discovery_url: Option<String>,
}

/// Generate a fresh installation id: 16 bytes from the OS CSPRNG encoded as
/// 26 unpadded lowercase RFC 4648 base32 characters.
///
/// The shape is the one `ActrUri`'s domain parser enforces for every
/// `<installation-id>.actr.alt` domain. Platforms call this once on first
/// use and own the persistence; later reads should be re-checked with
/// `validate_installation_id`.
pub fn generate_installation_id() -> String {
    use rand::RngCore;

    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    encode_base32_lowercase(&bytes)
}

/// Validate an installation id against the `.actr.alt` domain rules.
pub fn validate_installation_id(id: String) -> Result<(), CoreError> {
    let valid = id.len() == 26
        && id
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'2'..=b'7'));
    if valid {
        Ok(())
    } else {
        Err(CoreError::InvalidInstallationId)
    }
}

/// Lowercase RFC 4648 base32 without padding. 16 bytes yield exactly 26
/// characters, matching the shape `ActrUri::normalize_domain` accepts.
fn encode_base32_lowercase(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut output = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            output.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        output.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    output
}

/// Parse and validate an `actr://` Realm link or Actor address.
///
/// Layer-0 verification-funnel step: zero network access. Delegates to
/// `realm_link::open_realm_link`, which validates installation ownership and
/// rejects authority-only URIs, then dispatches a matching local Realm to
/// `open_local` and a DNS Realm to `discover_remote`. Both arms produce the
/// same shape; `is_local` tells the shell which follow-up flow applies.
pub fn parse_actr_uri(uri: String, installation_id: String) -> Result<ActrUriInfo, CoreError> {
    realm_link::open_realm_link::<ActrUriInfo, CoreError>(
        &uri,
        &installation_id,
        // Matching local Realm: resolved from local records only, never via
        // network discovery.
        |parsed| {
            Ok(ActrUriInfo {
                domain: parsed.domain().to_string(),
                realm_id: parsed.realm_id().map(ToOwned::to_owned),
                serial: parsed.serial().map(ToOwned::to_owned),
                is_local: true,
                discovery_url: parsed.discovery_url(),
            })
        },
        // DNS Realm: the discovery URL feeds the next funnel step.
        |parsed| {
            Ok(ActrUriInfo {
                domain: parsed.domain().to_string(),
                realm_id: parsed.realm_id().map(ToOwned::to_owned),
                serial: parsed.serial().map(ToOwned::to_owned),
                is_local: false,
                discovery_url: parsed.discovery_url(),
            })
        },
    )
}

impl From<AuthorityDescriptorError> for CoreError {
    fn from(error: AuthorityDescriptorError) -> Self {
        Self::AuthorityDescriptorInvalid(error.to_string())
    }
}

impl From<RealmProfileError> for CoreError {
    fn from(error: RealmProfileError) -> Self {
        match error {
            RealmProfileError::Retired(name) => Self::RealmRetired(name),
            other => Self::RealmProfileInvalid(other.to_string()),
        }
    }
}

/// A trusted realm signing key from an accepted authority descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmSigningKeyInfo {
    pub id: String,
    /// Base64 Ed25519 public key, as listed by the descriptor.
    pub public_key_base64: String,
    /// `active` or `retired`.
    pub status: String,
}

/// Validated authority descriptor for a Realm domain: the trusted realm
/// signing keys and the realms service location that feeds Realm Profile
/// discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityDiscoveryInfo {
    pub realms_href: String,
    /// Sequence of the accepted `realm_signing_keys` member.
    pub keys_sequence: u64,
    pub signing_keys: Vec<RealmSigningKeyInfo>,
    /// The accepted `realm_signing_keys` member serialized as JSON. The
    /// shell passes it back to `accept_realm_profile`, which rebuilds the
    /// keys that verify the profile signature.
    pub signing_keys_json: String,
}

/// Validate an authority descriptor response for `domain` and extract the
/// trusted realm signing keys plus the realms service href.
///
/// Layer-1 verification-funnel step. `body` must be the raw JSON text as
/// received over HTTPS (Web PKI is the first-accept trust root). The check
/// covers the descriptor structure, the `authority` binding to `domain`,
/// and the `realm_signing_keys` member including its detached Ed25519
/// signature; key pinning across visits is the caller's concern and is not
/// applied here yet.
pub fn accept_authority_descriptor(
    body: String,
    domain: String,
) -> Result<AuthorityDiscoveryInfo, CoreError> {
    let invalid = CoreError::AuthorityDescriptorInvalid;

    // Keep the raw member value: the signing payload is its JCS form, so it
    // must not round-trip through a typed struct before verification.
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| invalid(format!("not valid JSON: {error}")))?;
    let descriptor: AuthorityDescriptor = serde_json::from_value(value.clone())
        .map_err(|error| invalid(format!("malformed descriptor: {error}")))?;

    let requested = ActrUri::from_domain(&domain)
        .map_err(|error| invalid(format!("invalid discovery domain: {error}")))?;
    descriptor.validate_for(&requested)?;

    let realms_href = descriptor
        .require_service("realms")
        .map_err(CoreError::from)?
        .href
        .clone();

    let member = value
        .get("realm_signing_keys")
        .ok_or_else(|| invalid("descriptor carries no realm_signing_keys".to_string()))?;
    let (keys, _outcome) =
        authority::RealmSigningKeys::accept(member, None).map_err(CoreError::from)?;

    Ok(AuthorityDiscoveryInfo {
        realms_href,
        keys_sequence: keys.sequence,
        signing_keys_json: serde_json::to_string(&keys)
            .map_err(|error| invalid(format!("cannot serialize signing keys: {error}")))?,
        signing_keys: keys
            .keys
            .iter()
            .map(|key| RealmSigningKeyInfo {
                id: key.id.clone(),
                public_key_base64: key.public_key.clone(),
                status: format!("{:?}", key.status).to_lowercase(),
            })
            .collect(),
    })
}

/// A verified Realm Profile: the package target and endpoints that feed
/// package acquisition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmProfileInfo {
    /// Canonical Realm name `<realm_id>@<domain>`.
    pub realm_name: String,
    pub display_name: String,
    /// Always `active`; retired Realms are rejected.
    pub status: String,
    pub sequence: u64,
    /// HTTPS download URL of the signed `.actr` package.
    pub package_href: String,
    /// Lowercase hex SHA-256 the package must match.
    pub package_sha256: String,
    /// Actor type `publisher/program@version`.
    pub package_type: String,
    /// Issuer endpoint that serves Actor credentials.
    pub issuer_url: String,
    /// Actr Facility Provider ID; a membership client binds to it.
    pub provider_id: String,
}

/// Validate a Realm Profile response for `realm_name` and extract the
/// package target.
///
/// Layer-2 verification-funnel step. `body` must be the raw JSON text as
/// received; the signature is checked over the RFC 8785 JCS form of the
/// received document without its `signature` member, using the signing keys
/// accepted in the authority discovery step (`signing_keys_json`). A retired
/// Realm is rejected; key pinning across visits is not applied yet.
pub fn accept_realm_profile(
    body: String,
    realm_name: String,
    signing_keys_json: String,
) -> Result<RealmProfileInfo, CoreError> {
    let keys: RealmSigningKeys = serde_json::from_str(&signing_keys_json).map_err(|error| {
        CoreError::RealmProfileInvalid(format!("malformed signing keys: {error}"))
    })?;
    let profile = RealmProfile::verify(body.as_bytes(), &realm_name, &keys)?;
    if profile.realm.status != RealmStatus::Active {
        return Err(CoreError::RealmRetired(profile.realm.name.clone()));
    }
    // `RealmProfile::check` guarantees an active Realm carries a package.
    let package = profile
        .package
        .as_ref()
        .ok_or_else(|| CoreError::RealmProfileInvalid("active Realm has no package".to_string()))?;

    Ok(RealmProfileInfo {
        realm_name: profile.realm.name.clone(),
        display_name: profile.realm.display_name.clone(),
        status: "active".to_string(),
        sequence: profile.sequence,
        package_href: package.href.clone(),
        package_sha256: package.sha256.clone(),
        package_type: package.r#type.clone(),
        issuer_url: profile.issuer.url.clone(),
        provider_id: profile.provider.id.clone(),
    })
}

/// The Realm name carried by a `ri_` invite text.
///
/// Decoding happens here so the secret never leaves Rust: the shell only
/// learns the Realm name to show the confirmation UI and run discovery.
/// `join` re-decodes the same text internally when it consumes the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActrInviteInfo {
    pub realm_name: String,
}

pub fn decode_actr_invite(invite_text: String) -> Result<ActrInviteInfo, CoreError> {
    let invite = RealmInvite::decode(&invite_text).map_err(|error| {
        CoreError::AuthorityDescriptorInvalid(format!("invalid invite: {error}"))
    })?;
    Ok(ActrInviteInfo {
        realm_name: invite.realm,
    })
}

#[cfg(test)]
mod invite_tests {
    use super::*;

    #[test]
    fn decode_actr_invite_extracts_realm_name() {
        // 32 secret bytes, base64url without padding (43 characters).
        let text = RealmInvite::new(
            "echo@realm.example",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        )
        .encode();
        let info = decode_actr_invite(text).unwrap();
        assert_eq!(info.realm_name, "echo@realm.example");
    }

    #[test]
    fn decode_actr_invite_rejects_non_invite_text() {
        let error = decode_actr_invite("https://acme.example/realm".to_string()).unwrap_err();
        assert!(matches!(error, CoreError::AuthorityDescriptorInvalid(_)));
    }
}

/// A package signing key from a verified Publisher Profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherPackageKeyInfo {
    /// `actr-pub-` + 16 lowercase hex characters of the key digest.
    pub id: String,
    /// Base64 Ed25519 public key.
    pub public_key_base64: String,
    /// `active` or `retired`.
    pub status: String,
}

/// Verified Publisher Profile for a publisher domain: the package signing
/// keys that step ⑤ verifies the `.actr` package signature with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherProfileInfo {
    pub domain: String,
    pub profile_sequence: u64,
    /// RFC 3339; past this point the profile must be re-fetched.
    pub profile_expires_at: String,
    pub package_signing_keys: Vec<PublisherPackageKeyInfo>,
}

/// Extract the package signing keys from the descriptor a publisher domain
/// serves (`GET https://<domain>/.well-known/actr`).
///
/// Step ④ only fetches: no descriptor or profile validation happens here —
/// digest and signature verification of the package are step ⑤. The only
/// rejections are structural: no publisher member, or no signing keys in it.
pub fn accept_publisher_profile(
    body: String,
    domain: String,
) -> Result<PublisherProfileInfo, CoreError> {
    let invalid = |reason: String| {
        CoreError::AuthorityDescriptorInvalid(format!(
            "publisher information for '{domain}' is unusable: {reason}"
        ))
    };

    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| invalid(format!("not valid JSON: {error}")))?;
    let publisher = value
        .get("publisher")
        .ok_or_else(|| invalid("descriptor carries no publisher profile".into()))?;
    let keys = publisher
        .get("keys")
        .and_then(|keys| keys.as_array())
        .ok_or_else(|| invalid("publisher profile carries no signing keys".into()))?;

    let package_signing_keys: Vec<PublisherPackageKeyInfo> = keys
        .iter()
        .filter_map(|key| {
            Some(PublisherPackageKeyInfo {
                id: key.get("id")?.as_str()?.to_string(),
                public_key_base64: key.get("public_key")?.as_str()?.to_string(),
                status: key.get("status")?.as_str()?.to_string(),
            })
        })
        .collect();

    Ok(PublisherProfileInfo {
        domain,
        profile_sequence: publisher
            .get("sequence")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        profile_expires_at: publisher
            .get("profile_expires_at")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
        package_signing_keys,
    })
}

pub fn validate_actr_package_manifest(source: String) -> Result<(), CoreError> {
    actr_package::parse_manifest_toml(&source)
        .map(|_| ())
        .map_err(CoreError::from)
}

pub fn evaluate_contract(name: Option<String>, version: u32) -> ContractGateInfo {
    let contract = name.map(|name| Contract { name, version });
    let gate = actr_package::evaluate_contract(contract.as_ref());

    ContractGateInfo {
        status: gate.status.into(),
        blocked_reason: gate.blocked_reason,
    }
}

pub fn extract_app_id_from_url(url: String) -> Option<String> {
    actr_package::extract_app_id_from_url(&url)
}

pub fn normalize_path(path: String) -> String {
    actr_package::normalize_path(&path)
}

pub fn relative_path_from(absolute_path: String, base_dir: String) -> String {
    actr_package::relative_path_from(&absolute_path, &base_dir)
}

pub fn is_asset_path(path: String) -> bool {
    actr_package::is_asset_path(&path)
}

pub fn normalize_integrity_algorithm(raw: String) -> Option<String> {
    actr_package::normalize_integrity_algorithm(&raw).map(ToOwned::to_owned)
}

pub fn normalize_integrity_path(path: String) -> Option<String> {
    actr_package::normalize_integrity_path(&path)
}

pub fn normalize_sha256_hex(hash: String) -> Option<String> {
    actr_package::normalize_sha256_hex(&hash)
}

pub fn stable_content_fingerprint(input: String) -> String {
    let digest = Sha256::digest(input.as_bytes());
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn create_actr_integrity_plan(
    manifest_toml: String,
) -> Result<Option<PackageIntegrityPlanInfo>, CoreError> {
    let manifest = actr_package::parse_manifest_toml(&manifest_toml).map_err(CoreError::from)?;
    actr_package::build_integrity_plan(&manifest)
        .map(PackageIntegrityPlanInfo::from)
        .map(Some)
        .map_err(CoreError::from)
}

pub fn parse_system_app_type(raw: String) -> Option<String> {
    SystemAppType::try_from(raw.as_str())
        .ok()
        .map(|kind| kind.as_str().to_string())
}

pub fn system_app_info(raw: String) -> Result<SystemAppInfo, CoreError> {
    let kind = SystemAppType::try_from(raw.as_str())
        .map_err(|_| CoreError::UnknownSystemAppType(raw.clone()))?;

    Ok(SystemAppInfo {
        kind: kind.as_str().to_string(),
        title: kind.title().to_string(),
        about_url: kind.about_url().to_string(),
    })
}

pub fn system_app_type_bookmarks() -> String {
    SystemAppType::Bookmarks.as_str().to_string()
}

pub fn system_app_type_settings() -> String {
    SystemAppType::Settings.as_str().to_string()
}

pub fn system_app_type_global_search() -> String {
    SystemAppType::GlobalSearch.as_str().to_string()
}

pub fn parse_app_source_type(raw: String) -> Option<String> {
    AppSourceType::try_from(raw.as_str())
        .ok()
        .map(|kind| kind.as_str().to_string())
}

pub fn app_source_type_info(raw: String) -> Result<AppSourceTypeInfo, CoreError> {
    let kind = AppSourceType::try_from(raw.as_str())
        .map_err(|_| CoreError::UnknownAppSourceType(raw.clone()))?;

    Ok(AppSourceTypeInfo {
        kind: kind.as_str().to_string(),
        display_name: kind.display_name().to_string(),
    })
}

pub fn app_source_type_downloaded() -> String {
    AppSourceType::Downloaded.as_str().to_string()
}

pub fn app_source_type_local_dev() -> String {
    AppSourceType::LocalDev.as_str().to_string()
}

pub fn browser_search_engine_default_id() -> String {
    browser::DEFAULT_SEARCH_ENGINE_ID.to_string()
}

pub fn browser_search_engines() -> Vec<BrowserSearchEngineInfo> {
    browser::SEARCH_ENGINES
        .iter()
        .copied()
        .map(BrowserSearchEngineInfo::from)
        .collect()
}

pub fn browser_search_engine_info(engine_id: Option<String>) -> BrowserSearchEngineInfo {
    browser::search_engine_info(engine_id.as_deref()).into()
}

pub fn browser_search_engine_normalize_id(engine_id: Option<String>) -> String {
    browser::normalize_search_engine_id(engine_id.as_deref()).to_string()
}

pub fn browser_search_url(query: String, engine_id: Option<String>) -> String {
    browser::browser_search_url(&query, engine_id.as_deref())
}

pub fn browser_data_contract() -> BrowserDataContractInfo {
    BrowserDataContractInfo {
        schema_version: browser_data::BROWSER_DATA_SCHEMA_VERSION,
        max_history_items: browser_data::MAX_HISTORY_ITEMS as u32,
        max_bookmark_items: browser_data::MAX_BOOKMARK_ITEMS as u32,
        max_session_tabs: browser_data::MAX_SESSION_TABS as u32,
        max_title_length: browser_data::MAX_TITLE_LENGTH as u32,
        home_address: browser_data::HOME_ADDRESS.to_string(),
        history_address: browser_data::HISTORY_ADDRESS.to_string(),
        bookmarks_address: browser_data::BOOKMARKS_ADDRESS.to_string(),
    }
}

pub fn browser_history_snapshot_normalize_json(snapshot_json: String) -> String {
    browser_data::normalize_history_snapshot_json(&snapshot_json)
}

pub fn browser_bookmark_snapshot_normalize_json(snapshot_json: String) -> String {
    browser_data::normalize_bookmark_snapshot_json(&snapshot_json)
}

pub fn browser_session_snapshot_normalize_json(snapshot_json: String) -> String {
    browser_data::normalize_session_snapshot_json(&snapshot_json)
}

pub fn browser_session_home_snapshot_json() -> String {
    browser_data::home_session_snapshot_json()
}

pub fn browser_storage_schema() -> BrowserStorageSchemaInfo {
    BrowserStorageSchemaInfo {
        schema_version: browser_data::BROWSER_STORAGE_SCHEMA_VERSION,
        database_name: browser_data::BROWSER_STORAGE_DATABASE_NAME.to_string(),
        table_names: browser_storage_table_names(),
        statements: browser_storage_schema_statements(),
    }
}

pub fn browser_storage_schema_version() -> u32 {
    browser_data::BROWSER_STORAGE_SCHEMA_VERSION
}

pub fn browser_storage_database_name() -> String {
    browser_data::BROWSER_STORAGE_DATABASE_NAME.to_string()
}

pub fn browser_storage_schema_statements() -> Vec<String> {
    browser_data::browser_storage_schema_statements()
        .into_iter()
        .map(|statement| statement.trim().to_string())
        .collect()
}

pub fn browser_storage_schema_migration_plan_json() -> String {
    browser_data::browser_storage_schema_migration_plan_json()
}

pub fn browser_storage_table_names() -> Vec<String> {
    browser_data::browser_storage_table_names()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
}

pub fn browser_storage_repository_sql() -> BrowserStorageRepositorySqlInfo {
    let sql = browser_data::browser_storage_repository_sql();
    BrowserStorageRepositorySqlInfo {
        upsert_history_page: sql.upsert_history_page.trim().to_string(),
        insert_history_visit: sql.insert_history_visit.trim().to_string(),
        select_history_recent: sql.select_history_recent.trim().to_string(),
        delete_history_by_address: sql.delete_history_by_address.to_string(),
        delete_all_history: sql.delete_all_history.to_string(),
        upsert_bookmark: sql.upsert_bookmark.trim().to_string(),
        select_bookmarks_recent: sql.select_bookmarks_recent.trim().to_string(),
        delete_bookmark_by_address: sql.delete_bookmark_by_address.to_string(),
        delete_bookmark_by_id: sql.delete_bookmark_by_id.to_string(),
        delete_all_bookmarks: sql.delete_all_bookmarks.to_string(),
        upsert_session: sql.upsert_session.trim().to_string(),
        select_session: sql.select_session.to_string(),
        delete_session_tabs: sql.delete_session_tabs.to_string(),
        insert_session_tab: sql.insert_session_tab.trim().to_string(),
        select_session_tabs: sql.select_session_tabs.trim().to_string(),
    }
}

pub fn browser_storage_sync_sql() -> BrowserStorageSyncSqlInfo {
    let sql = browser_data::browser_storage_sync_sql();
    BrowserStorageSyncSqlInfo {
        select_dirty_history_pages: sql.select_dirty_history_pages.trim().to_string(),
        select_dirty_history_visits: sql.select_dirty_history_visits.trim().to_string(),
        select_dirty_bookmark_folders: sql.select_dirty_bookmark_folders.trim().to_string(),
        select_dirty_bookmarks: sql.select_dirty_bookmarks.trim().to_string(),
        select_dirty_sessions: sql.select_dirty_sessions.trim().to_string(),
        select_dirty_session_tabs: sql.select_dirty_session_tabs.trim().to_string(),
        apply_remote_history_page: sql.apply_remote_history_page.trim().to_string(),
        apply_remote_history_visit: sql.apply_remote_history_visit.trim().to_string(),
        apply_remote_bookmark_folder: sql.apply_remote_bookmark_folder.trim().to_string(),
        apply_remote_bookmark: sql.apply_remote_bookmark.trim().to_string(),
        apply_remote_session: sql.apply_remote_session.trim().to_string(),
        apply_remote_session_tab: sql.apply_remote_session_tab.trim().to_string(),
        mark_history_page_synced: sql.mark_history_page_synced.to_string(),
        mark_history_visit_synced: sql.mark_history_visit_synced.to_string(),
        mark_bookmark_folder_synced: sql.mark_bookmark_folder_synced.to_string(),
        mark_bookmark_synced: sql.mark_bookmark_synced.to_string(),
        mark_session_synced: sql.mark_session_synced.to_string(),
        mark_session_tab_synced: sql.mark_session_tab_synced.to_string(),
        upsert_sync_cursor: sql.upsert_sync_cursor.trim().to_string(),
        select_sync_cursors_for_scope: sql.select_sync_cursors_for_scope.to_string(),
    }
}

pub fn browser_storage_sync_conflict_policy_json() -> String {
    browser_data::browser_storage_sync_conflict_policy_json()
}

pub fn browser_storage_sync_merge_policy_json() -> String {
    browser_data::browser_storage_sync_merge_policy_json()
}

pub fn browser_storage_address_canonicalization_policy_json() -> String {
    browser_data::browser_storage_address_canonicalization_policy_json()
}

pub fn browser_storage_canonicalize_addresses_json(addresses_json: String) -> String {
    browser_data::browser_storage_canonicalize_addresses_json(&addresses_json)
}

pub fn browser_storage_bookmark_folder_tree_integrity_policy_json() -> String {
    browser_data::browser_storage_bookmark_folder_tree_integrity_policy_json()
}

pub fn browser_storage_bookmark_folder_tree_integrity_plan_json(folders_json: String) -> String {
    browser_data::browser_storage_bookmark_folder_tree_integrity_plan_json(&folders_json)
}

pub fn browser_storage_bookmark_restore_placement_policy_json() -> String {
    browser_data::browser_storage_bookmark_restore_placement_policy_json()
}

pub fn browser_storage_bookmark_restore_placement_plan_json(bookmarks_json: String) -> String {
    browser_data::browser_storage_bookmark_restore_placement_plan_json(&bookmarks_json)
}

pub fn browser_storage_bookmark_ownership_policy_json() -> String {
    browser_data::browser_storage_bookmark_ownership_policy_json()
}

pub fn browser_storage_bookmark_ownership_plan_json(bookmarks_json: String) -> String {
    browser_data::browser_storage_bookmark_ownership_plan_json(&bookmarks_json)
}

pub fn browser_storage_history_visit_aggregation_policy_json() -> String {
    browser_data::browser_storage_history_visit_aggregation_policy_json()
}

pub fn browser_storage_history_visit_aggregation_plan_json(visits_json: String) -> String {
    browser_data::browser_storage_history_visit_aggregation_plan_json(&visits_json)
}

pub fn browser_storage_session_tab_ordering_policy_json() -> String {
    browser_data::browser_storage_session_tab_ordering_policy_json()
}

pub fn browser_storage_session_tab_ordering_plan_json(tabs_json: String) -> String {
    browser_data::browser_storage_session_tab_ordering_plan_json(&tabs_json)
}

pub fn browser_storage_session_tab_id_plan_json(tabs_json: String) -> String {
    browser_data::browser_storage_session_tab_id_plan_json(&tabs_json)
}

pub fn browser_storage_session_active_tab_consistency_policy_json() -> String {
    browser_data::browser_storage_session_active_tab_consistency_policy_json()
}

pub fn browser_storage_session_active_tab_consistency_plan_json(sessions_json: String) -> String {
    browser_data::browser_storage_session_active_tab_consistency_plan_json(&sessions_json)
}

pub fn browser_storage_retention_pruning_policy_json() -> String {
    browser_data::browser_storage_retention_pruning_policy_json()
}

pub fn browser_storage_retention_pruning_plan_json(records_json: String) -> String {
    browser_data::browser_storage_retention_pruning_plan_json(&records_json)
}

pub fn browser_storage_data_model_json() -> String {
    browser_data::browser_storage_data_model_json()
}

pub fn normalize_browser_storage_sync_batch_json(batch_json: String) -> String {
    browser_data::normalize_browser_storage_sync_batch_json(&batch_json)
}

pub fn browser_storage_sync_batch_from_dirty_rows_json(
    scope: String,
    device_id: String,
    generated_at_unix_seconds: i64,
    rows_json: String,
) -> String {
    browser_data::browser_storage_sync_batch_from_dirty_rows_json(
        &scope,
        &device_id,
        generated_at_unix_seconds,
        &rows_json,
    )
}

pub fn browser_storage_sync_push_request_from_dirty_rows_json(
    scope: String,
    device_id: String,
    generated_at_unix_seconds: i64,
    rows_json: String,
) -> String {
    browser_data::browser_storage_sync_push_request_from_dirty_rows_json(
        &scope,
        &device_id,
        generated_at_unix_seconds,
        &rows_json,
    )
}

pub fn normalize_browser_storage_sync_ack_batch_json(ack_json: String) -> String {
    browser_data::normalize_browser_storage_sync_ack_batch_json(&ack_json)
}

pub fn normalize_browser_storage_sync_cursors_json(cursors_json: String) -> String {
    browser_data::normalize_browser_storage_sync_cursors_json(&cursors_json)
}

pub fn empty_browser_storage_sync_batch_json(
    scope: String,
    device_id: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::empty_browser_storage_sync_batch_json(
        &scope,
        &device_id,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_push_request_json(batch_json: String) -> String {
    browser_data::browser_storage_sync_push_request_json(&batch_json)
}

pub fn browser_storage_sync_pull_request_json(
    scope: String,
    device_id: String,
    generated_at_unix_seconds: i64,
    cursors_json: String,
    max_records_per_collection: u32,
) -> String {
    browser_data::browser_storage_sync_pull_request_json(
        &scope,
        &device_id,
        generated_at_unix_seconds,
        &cursors_json,
        max_records_per_collection,
    )
}

pub fn browser_storage_sync_cycle_response_json(
    remote_batch_json: String,
    ack_json: String,
) -> String {
    browser_data::browser_storage_sync_cycle_response_json(&remote_batch_json, &ack_json)
}

pub fn normalize_browser_storage_sync_cycle_response_json(response_json: String) -> String {
    browser_data::normalize_browser_storage_sync_cycle_response_json(&response_json)
}

pub fn browser_storage_sync_client_cycle_plan_json(response_json: String) -> String {
    browser_data::browser_storage_sync_client_cycle_plan_json(&response_json)
}

pub fn browser_storage_sync_client_ack_batch_after_remote_apply_json(
    response_json: String,
    remote_apply_result_json: String,
) -> String {
    browser_data::browser_storage_sync_client_ack_batch_after_remote_apply_json(
        &response_json,
        &remote_apply_result_json,
    )
}

pub fn browser_storage_sync_client_cycle_apply_result_json(
    response_json: String,
    remote_apply_result_json: String,
    ack_applied: bool,
) -> String {
    browser_data::browser_storage_sync_client_cycle_apply_result_json(
        &response_json,
        &remote_apply_result_json,
        ack_applied,
    )
}

pub fn browser_storage_sync_transport_contract_json() -> String {
    browser_data::browser_storage_sync_transport_contract_json()
}

pub fn browser_storage_sync_cloud_store_contract_json() -> String {
    browser_data::browser_storage_sync_cloud_store_contract_json()
}

pub fn browser_storage_sync_cloud_provider_adapter_contract_json() -> String {
    browser_data::browser_storage_sync_cloud_provider_adapter_contract_json()
}

pub fn browser_storage_sync_cloud_provider_connection_contract_json() -> String {
    browser_data::browser_storage_sync_cloud_provider_connection_contract_json()
}

pub fn browser_storage_sync_cloud_provider_error_policy_json() -> String {
    browser_data::browser_storage_sync_cloud_provider_error_policy_json()
}

pub fn browser_storage_sync_cloud_provider_adapter_plan_json(
    adapter_snapshot_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_provider_adapter_plan_json(
        &adapter_snapshot_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_store_capability_preflight_json(
    capabilities_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_store_capability_preflight_json(
        &capabilities_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_provider_connection_status_policy_json(
    provider_snapshot_json: String,
    retry_attempt: u32,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_provider_connection_status_policy_json(
        &provider_snapshot_json,
        retry_attempt,
        observed_at_unix_seconds,
    )
}

pub fn normalize_browser_sync_provider_settings_json(
    settings_json: String,
    is_root_absolute: bool,
    observed_at_unix_seconds: i64,
    fresh_device_id: String,
) -> String {
    browser_data::normalize_browser_sync_provider_settings_json(
        &settings_json,
        is_root_absolute,
        observed_at_unix_seconds,
        &fresh_device_id,
    )
}

pub fn browser_storage_sync_cloud_provider_error_classification_json(
    provider_errors_json: String,
    retry_attempt: u32,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_provider_error_classification_json(
        &provider_errors_json,
        retry_attempt,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_local_folder_capabilities_json() -> String {
    browser_data::browser_storage_sync_cloud_local_folder_capabilities_json()
}

pub fn browser_storage_sync_cloud_local_folder_capability_preflight_json(
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_local_folder_capability_preflight_json(
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_local_folder_compatibility_preflight_json(
    root_dir: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_local_folder_compatibility_preflight_json(
        &root_dir,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_data_model_json() -> String {
    browser_data::browser_storage_sync_cloud_data_model_json()
}

pub fn browser_storage_sync_cloud_default_manifest_json(
    scope: String,
    device_id: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_default_manifest_json(
        &scope,
        &device_id,
        generated_at_unix_seconds,
    )
}

pub fn normalize_browser_storage_sync_cloud_manifest_json(
    manifest_json: String,
    fallback_scope: String,
    fallback_device_id: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::normalize_browser_storage_sync_cloud_manifest_json(
        &manifest_json,
        &fallback_scope,
        &fallback_device_id,
        observed_at_unix_seconds,
    )
}

pub fn normalize_browser_storage_sync_cloud_device_json(
    device_json: String,
    fallback_scope: String,
    fallback_device_id: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::normalize_browser_storage_sync_cloud_device_json(
        &device_json,
        &fallback_scope,
        &fallback_device_id,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_log_object_json(
    collection: String,
    sequence: i64,
    batch_json: String,
) -> String {
    browser_data::browser_storage_sync_cloud_log_object_json(&collection, sequence, &batch_json)
}

pub fn normalize_browser_storage_sync_cloud_log_object_json(log_json: String) -> String {
    browser_data::normalize_browser_storage_sync_cloud_log_object_json(&log_json)
}

pub fn browser_storage_sync_cloud_snapshot_object_json(
    collection: String,
    generated_at_unix_seconds: i64,
    batch_json: String,
) -> String {
    browser_data::browser_storage_sync_cloud_snapshot_object_json(
        &collection,
        generated_at_unix_seconds,
        &batch_json,
    )
}

pub fn normalize_browser_storage_sync_cloud_snapshot_object_json(snapshot_json: String) -> String {
    browser_data::normalize_browser_storage_sync_cloud_snapshot_object_json(&snapshot_json)
}

pub fn browser_storage_sync_cloud_compatibility_status_json(
    document_kind: String,
    document_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_compatibility_status_json(
        &document_kind,
        &document_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_compatibility_preflight_json(
    documents_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_compatibility_preflight_json(
        &documents_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_schema_rollout_plan_json(
    compatibility_preflight_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_schema_rollout_plan_json(
        &compatibility_preflight_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_schema_migration_playbook_json(
    schema_rollout_plan_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_schema_migration_playbook_json(
        &schema_rollout_plan_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_recovery_snapshot_plan_json(
    schema_migration_playbook_json: String,
    compatibility_preflight_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_recovery_snapshot_plan_json(
        &schema_migration_playbook_json,
        &compatibility_preflight_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_recovery_snapshot_result_plan_json(
    recovery_snapshot_plan_json: String,
    write_results_json: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_recovery_snapshot_result_plan_json(
        &recovery_snapshot_plan_json,
        &write_results_json,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_local_folder_recovery_snapshot_round_json(
    root_dir: String,
    schema_migration_playbook_json: String,
    compatibility_preflight_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_local_folder_recovery_snapshot_round_json(
        &root_dir,
        &schema_migration_playbook_json,
        &compatibility_preflight_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_membership_plan_json(request_json: String) -> String {
    browser_data::browser_storage_sync_cloud_membership_plan_json(&request_json)
}

pub fn browser_storage_sync_cloud_membership_result_plan_json(
    membership_plan_json: String,
    write_results_json: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_membership_result_plan_json(
        &membership_plan_json,
        &write_results_json,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_local_folder_membership_round_json(
    root_dir: String,
    request_json: String,
) -> String {
    browser_data::browser_storage_sync_cloud_local_folder_membership_round_json(
        &root_dir,
        &request_json,
    )
}

pub fn browser_storage_sync_cloud_local_folder_compaction_round_json(
    root_dir: String,
    device_id: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_local_folder_compaction_round_json(
        &root_dir,
        &device_id,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_local_folder_cleanup_round_json(
    root_dir: String,
    cursors_json: String,
    device_id: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_local_folder_cleanup_round_json(
        &root_dir,
        &cursors_json,
        &device_id,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_local_folder_recovery_retention_round_json(
    root_dir: String,
    retain_snapshots: u32,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_local_folder_recovery_retention_round_json(
        &root_dir,
        retain_snapshots,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_push_plan_json(
    manifest_json: String,
    manifest_revision: String,
    device_json: String,
    device_revision: String,
    push_request_json: String,
) -> String {
    browser_data::browser_storage_sync_cloud_push_plan_json(
        &manifest_json,
        &manifest_revision,
        &device_json,
        &device_revision,
        &push_request_json,
    )
}

pub fn browser_storage_sync_cloud_push_result_plan_json(
    push_plan_json: String,
    write_results_json: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_push_result_plan_json(
        &push_plan_json,
        &write_results_json,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_pull_plan_json(
    manifest_json: String,
    device_id: String,
    cursors_json: String,
    generated_at_unix_seconds: i64,
    max_log_objects_per_collection: u32,
) -> String {
    browser_data::browser_storage_sync_cloud_pull_plan_json(
        &manifest_json,
        &device_id,
        &cursors_json,
        generated_at_unix_seconds,
        max_log_objects_per_collection,
    )
}

pub fn browser_storage_sync_cloud_pull_apply_plan_json(
    manifest_json: String,
    device_id: String,
    cursors_json: String,
    snapshots_json: String,
    logs_json: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_pull_apply_plan_json(
        &manifest_json,
        &device_id,
        &cursors_json,
        &snapshots_json,
        &logs_json,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_pull_continuation_plan_json(
    pull_plan_json: String,
    read_results_json: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_pull_continuation_plan_json(
        &pull_plan_json,
        &read_results_json,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_high_watermark_policy_json() -> String {
    browser_data::browser_storage_sync_cloud_high_watermark_policy_json()
}

pub fn browser_storage_sync_cloud_high_watermark_plan_json(
    manifest_json: String,
    snapshots_json: String,
    logs_json: String,
    cursors_json: String,
    device_id: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_high_watermark_plan_json(
        &manifest_json,
        &snapshots_json,
        &logs_json,
        &cursors_json,
        &device_id,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_cleanup_result_plan_json(
    high_watermark_plan_json: String,
    delete_results_json: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_cleanup_result_plan_json(
        &high_watermark_plan_json,
        &delete_results_json,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_compaction_plan_json(
    manifest_json: String,
    manifest_revision: String,
    snapshots_json: String,
    logs_json: String,
    device_id: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_compaction_plan_json(
        &manifest_json,
        &manifest_revision,
        &snapshots_json,
        &logs_json,
        &device_id,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_compaction_result_plan_json(
    compaction_plan_json: String,
    write_results_json: String,
    delete_results_json: String,
    generated_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_compaction_result_plan_json(
        &compaction_plan_json,
        &write_results_json,
        &delete_results_json,
        generated_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_cycle_client_plan_json(cycle_round_json: String) -> String {
    browser_data::browser_storage_sync_cloud_cycle_client_plan_json(&cycle_round_json)
}

pub fn browser_storage_sync_cloud_cycle_client_apply_result_json(
    cycle_round_json: String,
    step_results_json: String,
    remote_apply_result_json: String,
) -> String {
    browser_data::browser_storage_sync_cloud_cycle_client_apply_result_json(
        &cycle_round_json,
        &step_results_json,
        &remote_apply_result_json,
    )
}

pub fn browser_storage_sync_cloud_cycle_status_policy_json(
    cycle_round_json: String,
    retry_attempt: u32,
) -> String {
    browser_data::browser_storage_sync_cloud_cycle_status_policy_json(
        &cycle_round_json,
        retry_attempt,
    )
}

pub fn browser_storage_sync_cloud_membership_status_policy_json(
    membership_round_json: String,
    retry_attempt: u32,
) -> String {
    browser_data::browser_storage_sync_cloud_membership_status_policy_json(
        &membership_round_json,
        retry_attempt,
    )
}

pub fn browser_storage_sync_cloud_cycle_schedule_state_json(
    status_policy_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_cycle_schedule_state_json(
        &status_policy_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_membership_schedule_state_json(
    status_policy_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_membership_schedule_state_json(
        &status_policy_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_provider_diagnostics_json(
    capability_preflight_json: String,
    compatibility_preflight_json: String,
    provider_error_classification_json: String,
    cycle_status_policy_json: String,
    schedule_state_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_provider_diagnostics_json(
        &capability_preflight_json,
        &compatibility_preflight_json,
        &provider_error_classification_json,
        &cycle_status_policy_json,
        &schedule_state_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_observability_event_json(
    provider_diagnostics_json: String,
    attempt_plan_json: String,
    loop_plan_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_observability_event_json(
        &provider_diagnostics_json,
        &attempt_plan_json,
        &loop_plan_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_cycle_attempt_plan_json(
    cycle_round_json: String,
    retry_attempt: u32,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_cycle_attempt_plan_json(
        &cycle_round_json,
        retry_attempt,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_membership_attempt_plan_json(
    membership_round_json: String,
    retry_attempt: u32,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_cloud_membership_attempt_plan_json(
        &membership_round_json,
        retry_attempt,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_cloud_membership_loop_plan_json(
    membership_rounds_json: String,
    retry_attempt: u32,
    observed_at_unix_seconds: i64,
    max_attempts: u32,
) -> String {
    browser_data::browser_storage_sync_cloud_membership_loop_plan_json(
        &membership_rounds_json,
        retry_attempt,
        observed_at_unix_seconds,
        max_attempts,
    )
}

pub fn browser_storage_sync_cloud_cycle_loop_plan_json(
    cycle_rounds_json: String,
    retry_attempt: u32,
    observed_at_unix_seconds: i64,
    max_attempts: u32,
) -> String {
    browser_data::browser_storage_sync_cloud_cycle_loop_plan_json(
        &cycle_rounds_json,
        retry_attempt,
        observed_at_unix_seconds,
        max_attempts,
    )
}

pub fn browser_storage_sync_cloud_local_folder_cycle_round_json(
    root_dir: String,
    push_request_json: String,
    cursors_json: String,
    device_id: String,
    generated_at_unix_seconds: i64,
    max_log_objects_per_collection: u32,
) -> String {
    browser_data::browser_storage_sync_cloud_local_folder_cycle_round_json(
        &root_dir,
        &push_request_json,
        &cursors_json,
        &device_id,
        generated_at_unix_seconds,
        max_log_objects_per_collection,
    )
}

pub fn browser_storage_sync_cycle_request_plan_json(
    base_url: String,
    access_token: String,
    push_request_json: String,
    cursors_json: String,
) -> String {
    browser_data::browser_storage_sync_cycle_request_plan_json(
        &base_url,
        &access_token,
        &push_request_json,
        &cursors_json,
    )
}

pub fn browser_storage_sync_cycle_transport_result_json(
    status_code: i32,
    response_json: String,
    error_message: String,
) -> String {
    browser_data::browser_storage_sync_cycle_transport_result_json(
        status_code,
        &response_json,
        &error_message,
    )
}

pub fn browser_storage_sync_status_policy_json(
    classification: String,
    should_retry: bool,
    client_action: String,
    retry_attempt: u32,
) -> String {
    browser_data::browser_storage_sync_status_policy_json(
        &classification,
        should_retry,
        &client_action,
        retry_attempt,
    )
}

pub fn browser_storage_sync_schedule_state_json(
    status_policy_json: String,
    observed_at_unix_seconds: i64,
) -> String {
    browser_data::browser_storage_sync_schedule_state_json(
        &status_policy_json,
        observed_at_unix_seconds,
    )
}

pub fn browser_storage_sync_scheduler_status_json(scheduler_snapshot_json: String) -> String {
    browser_data::browser_storage_sync_scheduler_status_json(&scheduler_snapshot_json)
}

pub fn browser_storage_sync_ack_plan_json(ack_json: String) -> String {
    browser_data::browser_storage_sync_ack_plan_json(&ack_json)
}

pub fn browser_storage_sync_remote_apply_plan_json(batch_json: String) -> String {
    browser_data::browser_storage_sync_remote_apply_plan_json(&batch_json)
}

pub fn browser_storage_sync_remote_apply_result_json(
    plan_json: String,
    result_json: String,
) -> String {
    browser_data::browser_storage_sync_remote_apply_result_json(&plan_json, &result_json)
}

pub fn browser_storage_sync_server_merge_plan_json(push_request_json: String) -> String {
    browser_data::browser_storage_sync_server_merge_plan_json(&push_request_json)
}

pub fn browser_storage_sync_server_merge_result_json(
    plan_json: String,
    result_json: String,
) -> String {
    browser_data::browser_storage_sync_server_merge_result_json(&plan_json, &result_json)
}

pub fn supported_contract_name() -> String {
    actr_package::SUPPORTED_CONTRACT_NAME.to_string()
}

pub fn supported_contract_version() -> u32 {
    actr_package::SUPPORTED_CONTRACT_VERSION
}

pub fn core_schema_version() -> u32 {
    persistence::CORE_SCHEMA_VERSION
}

pub fn core_schema_statements() -> Vec<String> {
    persistence::schema_statements()
        .into_iter()
        .map(|statement| statement.trim().to_string())
        .collect()
}

pub fn core_schema_table_names() -> Vec<String> {
    persistence::table_names()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
}

pub fn default_tags() -> Vec<DefaultTagInfo> {
    persistence::DEFAULT_TAGS
        .iter()
        .map(|tag| DefaultTagInfo {
            name: tag.name.to_string(),
            color: tag.color.to_string(),
            is_system: tag.is_system,
        })
        .collect()
}

pub fn default_system_tags() -> Vec<DefaultTagInfo> {
    persistence::DEFAULT_TAGS
        .iter()
        .filter(|tag| tag.is_system)
        .map(|tag| DefaultTagInfo {
            name: tag.name.to_string(),
            color: tag.color.to_string(),
            is_system: tag.is_system,
        })
        .collect()
}

pub fn default_tag_colors() -> Vec<String> {
    DomainTag::DEFAULT_COLORS
        .iter()
        .map(ToString::to_string)
        .collect()
}

pub fn core_history_repository_sql() -> CoreHistoryRepositorySqlInfo {
    CoreHistoryRepositorySqlInfo {
        insert_visit: persistence::query::history::INSERT_VISIT.to_string(),
        select_recent: persistence::query::history::SELECT_RECENT.to_string(),
        select_all: persistence::query::history::SELECT_ALL.to_string(),
        search_by_title: persistence::query::history::SEARCH_BY_TITLE.to_string(),
        delete_by_id: persistence::query::history::DELETE_BY_ID.to_string(),
        delete_all: persistence::query::history::DELETE_ALL.to_string(),
    }
}

pub fn core_history_item_from_row(row: CoreHistoryItemRowInfo) -> CoreHistoryItemInfo {
    CoreHistoryItemInfo {
        id: row.id,
        title: row.title,
        app_url: row.app_url,
        app_id: row.app_id,
        visit_time_seconds: row.visit_time_seconds,
    }
}

pub fn core_history_insert_visit_params(
    id: String,
    title: String,
    app_url: Option<String>,
    app_id: Option<String>,
    visit_time_seconds: i64,
) -> Vec<CoreSqlValueInfo> {
    vec![
        sql_text(id),
        sql_text(title),
        sql_optional_text(app_url),
        sql_optional_text(app_id),
        sql_integer(visit_time_seconds),
    ]
}

pub fn core_history_search_title_param(query: String) -> Option<String> {
    search_like_param(&query)
}

pub fn core_tag_repository_sql() -> CoreTagRepositorySqlInfo {
    CoreTagRepositorySqlInfo {
        count_all: persistence::query::tags::COUNT_ALL.to_string(),
        select_all: persistence::query::tags::SELECT_ALL.to_string(),
        select_user: persistence::query::tags::SELECT_USER.to_string(),
        insert_tag: persistence::query::tags::INSERT_TAG.to_string(),
        update_tag: persistence::query::tags::UPDATE_TAG.to_string(),
        delete_user_tag_by_id: persistence::query::tags::DELETE_USER_TAG_BY_ID.to_string(),
        select_by_bookmark_id: persistence::query::tags::SELECT_BY_BOOKMARK_ID
            .trim()
            .to_string(),
        delete_bookmark_tags_by_bookmark_id:
            persistence::query::tags::DELETE_BOOKMARK_TAGS_BY_BOOKMARK_ID.to_string(),
        insert_bookmark_tag: persistence::query::tags::INSERT_BOOKMARK_TAG.to_string(),
        insert_or_ignore_bookmark_tag: persistence::query::tags::INSERT_OR_IGNORE_BOOKMARK_TAG
            .to_string(),
        delete_bookmark_tag: persistence::query::tags::DELETE_BOOKMARK_TAG.to_string(),
    }
}

pub fn core_tag_from_row(row: CoreTagRowInfo) -> CoreTagInfo {
    CoreTagInfo {
        id: row.id,
        name: row.name,
        color: row.color,
        is_system: row.is_system_value != 0,
    }
}

pub fn core_tag_insert_params(
    id: String,
    name: String,
    color: Option<String>,
    is_system: bool,
) -> Vec<CoreSqlValueInfo> {
    vec![
        sql_text(id),
        sql_text(name),
        sql_optional_text(color),
        sql_bool(is_system),
    ]
}

pub fn core_tag_update_params(
    name: String,
    color: Option<String>,
    id: String,
) -> Vec<CoreSqlValueInfo> {
    vec![sql_text(name), sql_optional_text(color), sql_text(id)]
}

fn search_like_param(query: &str) -> Option<String> {
    if query.is_empty() {
        None
    } else {
        Some(format!("%{}%", query.to_lowercase()))
    }
}

pub fn core_bookmark_repository_sql() -> CoreBookmarkRepositorySqlInfo {
    CoreBookmarkRepositorySqlInfo {
        insert_bookmark: persistence::query::bookmarks::INSERT_BOOKMARK
            .trim()
            .to_string(),
        update_bookmark: persistence::query::bookmarks::UPDATE_BOOKMARK
            .trim()
            .to_string(),
        delete_by_id: persistence::query::bookmarks::DELETE_BY_ID.to_string(),
        select_all: persistence::query::bookmarks::SELECT_ALL.to_string(),
        select_by_tag: persistence::query::bookmarks::SELECT_BY_TAG
            .trim()
            .to_string(),
        search_by_title_or_notes: persistence::query::bookmarks::SEARCH_BY_TITLE_OR_NOTES
            .trim()
            .to_string(),
        count_by_app_url: persistence::query::bookmarks::COUNT_BY_APP_URL.to_string(),
        select_by_app_url: persistence::query::bookmarks::SELECT_BY_APP_URL.to_string(),
        update_last_used: persistence::query::bookmarks::UPDATE_LAST_USED.to_string(),
        delete_null_app_url: persistence::query::bookmarks::DELETE_NULL_APP_URL.to_string(),
        delete_duplicates_by_app_url: persistence::query::bookmarks::DELETE_DUPLICATES_BY_APP_URL
            .trim()
            .to_string(),
        delete_bookmark_tags: persistence::query::bookmarks::DELETE_BOOKMARK_TAGS.to_string(),
        delete_all: persistence::query::bookmarks::DELETE_ALL.to_string(),
        vacuum: persistence::query::bookmarks::VACUUM.to_string(),
    }
}

pub fn core_bookmark_from_row(row: CoreBookmarkRowInfo) -> CoreBookmarkInfo {
    CoreBookmarkInfo {
        id: row.id,
        title: row.title,
        app_url: row.app_url,
        app_id: row.app_id,
        created_at_seconds: row.created_at_seconds,
        last_used_at_seconds: row.last_used_at_seconds,
        publisher: row.publisher,
        notes: row.notes,
        favicon: row.favicon,
    }
}

pub fn core_bookmark_insert_params(bookmark: CoreBookmarkInfo) -> Vec<CoreSqlValueInfo> {
    vec![
        sql_text(bookmark.id),
        sql_text(bookmark.title),
        sql_optional_text(bookmark.app_url),
        sql_optional_text(bookmark.app_id),
        sql_integer(bookmark.created_at_seconds),
        sql_optional_integer(bookmark.last_used_at_seconds),
        sql_optional_text(bookmark.publisher),
        sql_optional_text(bookmark.notes),
        sql_optional_text(bookmark.favicon),
    ]
}

pub fn core_bookmark_update_params(bookmark: CoreBookmarkInfo) -> Vec<CoreSqlValueInfo> {
    vec![
        sql_text(bookmark.title),
        sql_optional_text(bookmark.app_url),
        sql_optional_text(bookmark.app_id),
        sql_optional_integer(bookmark.last_used_at_seconds),
        sql_optional_text(bookmark.publisher),
        sql_optional_text(bookmark.notes),
        sql_optional_text(bookmark.favicon),
        sql_text(bookmark.id),
    ]
}

pub fn core_bookmark_search_params(query: String) -> Vec<String> {
    search_like_param(&query)
        .map(|param| vec![param.clone(), param])
        .unwrap_or_default()
}

pub fn core_app_repository_sql() -> CoreAppRepositorySqlInfo {
    CoreAppRepositorySqlInfo {
        insert_or_replace_app: persistence::query::apps::INSERT_OR_REPLACE_APP
            .trim()
            .to_string(),
        update_app: persistence::query::apps::UPDATE_APP.trim().to_string(),
        delete_by_id: persistence::query::apps::DELETE_BY_ID.to_string(),
        delete_by_bundle_id: persistence::query::apps::DELETE_BY_BUNDLE_ID.to_string(),
        select_all: persistence::query::apps::SELECT_ALL.to_string(),
        select_by_source_type: persistence::query::apps::SELECT_BY_SOURCE_TYPE.to_string(),
        select_pinned: persistence::query::apps::SELECT_PINNED.to_string(),
        select_by_bundle_id: persistence::query::apps::SELECT_BY_BUNDLE_ID.to_string(),
        select_by_id: persistence::query::apps::SELECT_BY_ID.to_string(),
        count_by_bundle_id: persistence::query::apps::COUNT_BY_BUNDLE_ID.to_string(),
        update_last_used_by_bundle_id: persistence::query::apps::UPDATE_LAST_USED_BY_BUNDLE_ID
            .to_string(),
        toggle_pinned_by_id: persistence::query::apps::TOGGLE_PINNED_BY_ID.to_string(),
        search_by_name_description_or_bundle_id:
            persistence::query::apps::SEARCH_BY_NAME_DESCRIPTION_OR_BUNDLE_ID
                .trim()
                .to_string(),
        delete_all: persistence::query::apps::DELETE_ALL.to_string(),
        vacuum: persistence::query::apps::VACUUM.to_string(),
    }
}

pub fn core_installed_app_from_row(row: CoreInstalledAppRowInfo) -> Option<CoreInstalledAppInfo> {
    let id = row.id?;
    let name = row.name?;
    let bundle_id = row.bundle_id?;
    let source_type = AppSourceType::try_from(row.source_type?.as_str())
        .ok()?
        .as_str()
        .to_string();
    let source_path = row.source_path?;
    let installed_at_seconds = row.installed_at_seconds?;

    Some(CoreInstalledAppInfo {
        id,
        name,
        bundle_id,
        version: row.version,
        description: row.description,
        source_type,
        source_path,
        icon: row.icon,
        installed_at_seconds,
        last_used_at_seconds: row.last_used_at_seconds,
        is_pinned: row.is_pinned_value != 0,
    })
}

pub fn core_installed_app_insert_params(
    app: CoreInstalledAppInfo,
) -> Result<Vec<CoreSqlValueInfo>, CoreError> {
    let source_type = canonical_app_source_type(&app.source_type)?;

    Ok(vec![
        sql_text(app.id),
        sql_text(app.name),
        sql_text(app.bundle_id),
        sql_optional_text(app.version),
        sql_optional_text(app.description),
        sql_text(source_type),
        sql_text(app.source_path),
        sql_optional_text(app.icon),
        sql_integer(app.installed_at_seconds),
        sql_optional_integer(app.last_used_at_seconds),
        sql_bool(app.is_pinned),
    ])
}

pub fn core_installed_app_update_params(
    app: CoreInstalledAppInfo,
) -> Result<Vec<CoreSqlValueInfo>, CoreError> {
    canonical_app_source_type(&app.source_type)?;

    Ok(vec![
        sql_text(app.name),
        sql_optional_text(app.version),
        sql_optional_text(app.description),
        sql_text(app.source_path),
        sql_optional_text(app.icon),
        sql_optional_integer(app.last_used_at_seconds),
        sql_bool(app.is_pinned),
        sql_text(app.id),
    ])
}

pub fn core_app_search_params(query: String) -> Vec<String> {
    search_like_param(&query)
        .map(|param| vec![param.clone(), param.clone(), param])
        .unwrap_or_default()
}

fn canonical_app_source_type(source_type: &str) -> Result<String, CoreError> {
    AppSourceType::try_from(source_type)
        .map(|kind| kind.as_str().to_string())
        .map_err(|_| CoreError::UnknownAppSourceType(source_type.to_string()))
}

fn sql_null() -> CoreSqlValueInfo {
    CoreSqlValueInfo {
        kind: "null".to_string(),
        text: None,
        integer: None,
        real: None,
    }
}

fn sql_text(value: String) -> CoreSqlValueInfo {
    CoreSqlValueInfo {
        kind: "text".to_string(),
        text: Some(value),
        integer: None,
        real: None,
    }
}

fn sql_optional_text(value: Option<String>) -> CoreSqlValueInfo {
    value.map(sql_text).unwrap_or_else(sql_null)
}

fn sql_integer(value: i64) -> CoreSqlValueInfo {
    CoreSqlValueInfo {
        kind: "integer".to_string(),
        text: None,
        integer: Some(value),
        real: None,
    }
}

fn sql_optional_integer(value: Option<i64>) -> CoreSqlValueInfo {
    value.map(sql_integer).unwrap_or_else(sql_null)
}

fn sql_bool(value: bool) -> CoreSqlValueInfo {
    sql_integer(if value { 1 } else { 0 })
}

pub fn runtime_app_status_alive() -> String {
    runtime_persistence::app_status::ALIVE.to_string()
}

pub fn runtime_app_status_dead() -> String {
    runtime_persistence::app_status::DEAD.to_string()
}

pub fn default_runtime_app_status() -> String {
    runtime_persistence::default_app_status().to_string()
}

pub fn normalize_runtime_app_status(value: String) -> Option<String> {
    runtime_persistence::normalize_app_status(&value).map(ToOwned::to_owned)
}

pub fn active_tab_from_row(row: ActiveTabRowInfo) -> ActiveTabInfo {
    ActiveTabInfo {
        id: row.id,
        name: row.name,
        icon: row.icon,
        screenshot: row.screenshot,
        has_unread: row.has_unread,
        url: row.url,
        is_marked: row.is_marked,
        panel_visible: false,
        status: canonical_runtime_app_status_or_default(&row.status),
    }
}

pub fn active_tab_to_row(tab: ActiveTabInfo, sort_order: i32) -> ActiveTabRowInfo {
    ActiveTabRowInfo {
        id: tab.id,
        name: tab.name,
        icon: tab.icon,
        screenshot: tab.screenshot,
        has_unread: tab.has_unread,
        url: tab.url,
        is_marked: tab.is_marked,
        status: canonical_runtime_app_status_or_default(&tab.status),
        sort_order,
    }
}

pub fn active_tab_rows_from_tabs(tabs: Vec<ActiveTabInfo>) -> Vec<ActiveTabRowInfo> {
    tabs.into_iter()
        .enumerate()
        .map(|(index, tab)| active_tab_to_row(tab, i32::try_from(index).unwrap_or(i32::MAX)))
        .collect()
}

pub fn runtime_tab_update_from_fields(
    tab_id: Option<String>,
    title: Option<String>,
    icon: Option<String>,
    app_url: Option<String>,
    status: Option<String>,
) -> Option<RuntimeTabUpdateInfo> {
    tab_id.map(|tab_id| RuntimeTabUpdateInfo {
        tab_id,
        title,
        icon,
        app_url,
        status: status.map(|status| canonical_runtime_app_status_or_default(&status)),
    })
}

pub fn runtime_bridge_tab_id_key() -> String {
    RUNTIME_BRIDGE_KEY_TAB_ID.to_string()
}

pub fn runtime_bridge_title_key() -> String {
    RUNTIME_BRIDGE_KEY_TITLE.to_string()
}

pub fn runtime_bridge_icon_key() -> String {
    RUNTIME_BRIDGE_KEY_ICON.to_string()
}

pub fn runtime_bridge_app_url_key() -> String {
    RUNTIME_BRIDGE_KEY_APP_URL.to_string()
}

pub fn runtime_bridge_status_key() -> String {
    RUNTIME_BRIDGE_KEY_STATUS.to_string()
}

pub fn runtime_bridge_unread_key() -> String {
    RUNTIME_BRIDGE_KEY_UNREAD.to_string()
}

pub fn runtime_bridge_marked_key() -> String {
    RUNTIME_BRIDGE_KEY_MARKED.to_string()
}

pub fn runtime_bridge_is_bookmarked_key() -> String {
    RUNTIME_BRIDGE_KEY_IS_BOOKMARKED.to_string()
}

pub fn runtime_bridge_visible_key() -> String {
    RUNTIME_BRIDGE_KEY_VISIBLE.to_string()
}

pub fn runtime_bridge_container_width_key() -> String {
    RUNTIME_BRIDGE_KEY_CONTAINER_WIDTH.to_string()
}

pub fn runtime_bridge_container_height_key() -> String {
    RUNTIME_BRIDGE_KEY_CONTAINER_HEIGHT.to_string()
}

pub fn runtime_tab_entered_fullscreen_event_name() -> String {
    RUNTIME_EVENT_TAB_ENTERED_FULLSCREEN.to_string()
}

pub fn runtime_tab_exited_fullscreen_event_name() -> String {
    RUNTIME_EVENT_TAB_EXITED_FULLSCREEN.to_string()
}

pub fn runtime_tab_bookmark_changed_event_name() -> String {
    RUNTIME_EVENT_TAB_BOOKMARK_CHANGED.to_string()
}

pub fn runtime_extension_panel_visible_change_event_name() -> String {
    RUNTIME_EVENT_EXTENSION_PANEL_VISIBLE_CHANGE.to_string()
}

pub fn runtime_container_size_change_event_name() -> String {
    RUNTIME_EVENT_CONTAINER_SIZE_CHANGE.to_string()
}

pub fn runtime_search_input_changed_event_name() -> String {
    RUNTIME_EVENT_SEARCH_INPUT_CHANGED.to_string()
}

pub fn runtime_auth_context_client_id_key() -> String {
    RUNTIME_AUTH_CONTEXT_KEY_CLIENT_ID.to_string()
}

pub fn runtime_auth_context_app_id_key() -> String {
    RUNTIME_AUTH_CONTEXT_KEY_APP_ID.to_string()
}

pub fn runtime_auth_context_tab_id_key() -> String {
    RUNTIME_AUTH_CONTEXT_KEY_TAB_ID.to_string()
}

pub fn runtime_auth_context_app_url_key() -> String {
    RUNTIME_AUTH_CONTEXT_KEY_APP_URL.to_string()
}

pub fn auth_environment_dev() -> String {
    auth::AUTH_ENVIRONMENT_DEV.to_string()
}

pub fn auth_environment_beta() -> String {
    auth::AUTH_ENVIRONMENT_BETA.to_string()
}

pub fn auth_environment_prod() -> String {
    auth::AUTH_ENVIRONMENT_PROD.to_string()
}

pub fn auth_environment_from_url(url: String) -> String {
    auth::auth_environment_from_url(&url).to_string()
}

pub fn auth_environment_from_app_url(url: String) -> String {
    auth::auth_environment_from_app_url(&url).to_string()
}

pub fn auth_environment_storage_suffix(environment: String) -> String {
    auth::auth_environment_storage_suffix(&environment).to_string()
}

pub fn auth_account_base_url(environment: String) -> String {
    auth::auth_account_base_url(&environment).to_string()
}

pub fn auth_resource_uploader_base_url(environment: String) -> String {
    auth::auth_resource_uploader_base_url(&environment).to_string()
}

pub fn auth_access_control_path(app_id: String, user_id: String) -> String {
    auth::auth_access_control_path(&app_id, &user_id)
}

pub fn auth_client_app_path(app_id: String) -> String {
    auth::auth_client_app_path(&app_id)
}

pub fn auth_resource_upload_token_path() -> String {
    auth::AUTH_RESOURCE_UPLOAD_TOKEN_PATH.to_string()
}

pub fn auth_access_control_url_from_app_url(
    app_url: String,
    app_id: String,
    user_id: String,
) -> Option<String> {
    auth::auth_access_control_url_from_app_url(&app_url, &app_id, &user_id)
}

pub fn auth_is_authorized_code(code: i32) -> bool {
    auth::auth_is_authorized_code(code)
}

pub fn auth_access_control_status(success: bool, has_code: bool, code: i32) -> String {
    auth::auth_access_control_status(success, has_code.then_some(code)).to_string()
}

pub fn auth_api_response_success(code: i32) -> bool {
    auth::auth_api_response_success(code)
}

pub fn auth_is_client_app_not_found_code(code: i32) -> bool {
    auth::auth_is_client_app_not_found_code(code)
}

pub fn auth_is_client_app_not_found_message(message: String) -> bool {
    auth::auth_is_client_app_not_found_message(&message)
}

pub fn runtime_login_result_event_name() -> String {
    RUNTIME_LOGIN_RESULT_EVENT_NAME.to_string()
}

pub fn runtime_login_result_type_key() -> String {
    RUNTIME_LOGIN_RESULT_KEY_TYPE.to_string()
}

pub fn runtime_login_result_code_key() -> String {
    RUNTIME_LOGIN_RESULT_KEY_CODE.to_string()
}

pub fn runtime_login_result_id_token_key() -> String {
    RUNTIME_LOGIN_RESULT_KEY_ID_TOKEN.to_string()
}

pub fn runtime_login_result_error_key() -> String {
    RUNTIME_LOGIN_RESULT_KEY_ERROR.to_string()
}

pub fn runtime_login_result_message_key() -> String {
    RUNTIME_LOGIN_RESULT_KEY_MESSAGE.to_string()
}

pub fn runtime_login_type_wechat() -> String {
    RUNTIME_LOGIN_TYPE_WECHAT.to_string()
}

pub fn runtime_login_type_apple() -> String {
    RUNTIME_LOGIN_TYPE_APPLE.to_string()
}

pub fn runtime_login_type_google() -> String {
    RUNTIME_LOGIN_TYPE_GOOGLE.to_string()
}

pub fn runtime_tab_props_unified_script_key() -> String {
    RUNTIME_TAB_PROPS_KEY_UNIFIED_SCRIPT.to_string()
}

pub fn runtime_tab_props_right_panel_default_visible_key() -> String {
    RUNTIME_TAB_PROPS_KEY_RIGHT_PANEL_DEFAULT_VISIBLE.to_string()
}

pub fn runtime_tab_props_id_key() -> String {
    RUNTIME_TAB_PROPS_KEY_ID.to_string()
}

pub fn runtime_tab_props_background_color_key() -> String {
    RUNTIME_TAB_PROPS_KEY_BACKGROUND_COLOR.to_string()
}

pub fn runtime_tab_props_reload_key_key() -> String {
    RUNTIME_TAB_PROPS_KEY_RELOAD_KEY.to_string()
}

pub fn runtime_tab_props_html_content_key() -> String {
    RUNTIME_TAB_PROPS_KEY_HTML_CONTENT.to_string()
}

pub fn runtime_root_props_tab_key() -> String {
    RUNTIME_ROOT_PROPS_KEY_TAB.to_string()
}

pub fn runtime_root_props_tabs_key() -> String {
    RUNTIME_ROOT_PROPS_KEY_TABS.to_string()
}

pub fn runtime_root_props_single_tab_mode_key() -> String {
    RUNTIME_ROOT_PROPS_KEY_SINGLE_TAB_MODE.to_string()
}

pub fn runtime_root_props_current_tab_id_key() -> String {
    RUNTIME_ROOT_PROPS_KEY_CURRENT_TAB_ID.to_string()
}

pub fn runtime_root_props_is_left_panel_visible_key() -> String {
    RUNTIME_ROOT_PROPS_KEY_IS_LEFT_PANEL_VISIBLE.to_string()
}

pub fn runtime_root_props_is_right_panel_visible_key() -> String {
    RUNTIME_ROOT_PROPS_KEY_IS_RIGHT_PANEL_VISIBLE.to_string()
}

pub fn runtime_tab_unread_from_fields(
    tab_id: Option<String>,
    unread: Option<bool>,
) -> Option<RuntimeTabUnreadInfo> {
    Some(RuntimeTabUnreadInfo {
        tab_id: tab_id?,
        unread: unread?,
    })
}

pub fn runtime_tab_bookmark_status_from_fields(
    tab_id: Option<String>,
    marked: Option<bool>,
) -> Option<RuntimeTabBookmarkStatusInfo> {
    Some(RuntimeTabBookmarkStatusInfo {
        tab_id: tab_id?,
        marked: marked?,
    })
}

pub fn runtime_tab_bookmark_event(
    tab_id: String,
    is_bookmarked: bool,
) -> RuntimeTabBookmarkEventInfo {
    RuntimeTabBookmarkEventInfo {
        tab_id,
        is_bookmarked,
    }
}

pub fn runtime_tab_fullscreen_event(tab_id: String) -> RuntimeTabFullscreenEventInfo {
    RuntimeTabFullscreenEventInfo { tab_id }
}

pub fn runtime_extension_panel_visible_event(
    tab_id: String,
    visible: bool,
) -> RuntimeExtensionPanelVisibleEventInfo {
    RuntimeExtensionPanelVisibleEventInfo { tab_id, visible }
}

pub fn runtime_container_size_change_event(
    container_width: i32,
    container_height: i32,
) -> RuntimeContainerSizeChangeEventInfo {
    RuntimeContainerSizeChangeEventInfo {
        container_width,
        container_height,
    }
}

pub fn runtime_tab_from_active_tab(tab: ActiveTabInfo) -> RuntimeTabInfo {
    RuntimeTabInfo {
        unified_script: String::new(),
        right_panel_default_visible: false,
        id: tab.id,
        background_color: "#FFFFFF".to_string(),
        reload_key: 0,
        html_content: String::new(),
        title: tab.name,
    }
}

pub fn runtime_root_props_from_active_tabs(
    tab: ActiveTabInfo,
    tabs: Vec<ActiveTabInfo>,
    container_height: i32,
    container_width: i32,
) -> RuntimeRootPropsInfo {
    let app_url = Some(tab.url.clone());
    let runtime_tab = runtime_tab_from_active_tab(tab);
    let runtime_tabs = tabs.into_iter().map(runtime_tab_from_active_tab).collect();

    runtime_root_props_from_runtime_tabs(
        runtime_tab,
        runtime_tabs,
        false,
        false,
        container_height,
        container_width,
        app_url,
    )
}

pub fn runtime_root_props_from_runtime_tabs(
    tab: RuntimeTabInfo,
    tabs: Vec<RuntimeTabInfo>,
    is_left_panel_visible: bool,
    is_right_panel_visible: bool,
    container_height: i32,
    container_width: i32,
    app_url: Option<String>,
) -> RuntimeRootPropsInfo {
    RuntimeRootPropsInfo {
        current_tab_id: tab.id.clone(),
        tab,
        is_left_panel_visible,
        tabs,
        single_tab_mode: true,
        is_right_panel_visible,
        container_height,
        container_width,
        app_url,
    }
}

pub fn runtime_system_root_props(
    system_app_type: String,
) -> Result<RuntimeSystemRootPropsInfo, CoreError> {
    let kind = SystemAppType::try_from(system_app_type.as_str())
        .map_err(|_| CoreError::UnknownSystemAppType(system_app_type.clone()))?;

    Ok(RuntimeSystemRootPropsInfo {
        tab: RuntimeSystemTabInfo {
            system_app_type: kind.as_str().to_string(),
        },
        single_tab_mode: true,
    })
}

fn canonical_runtime_app_status_or_default(value: &str) -> String {
    runtime_persistence::normalize_app_status(value)
        .unwrap_or_else(runtime_persistence::default_app_status)
        .to_string()
}

const RUNTIME_BRIDGE_KEY_TAB_ID: &str = "tabId";
const RUNTIME_BRIDGE_KEY_TITLE: &str = "title";
const RUNTIME_BRIDGE_KEY_ICON: &str = "icon";
const RUNTIME_BRIDGE_KEY_APP_URL: &str = "appUrl";
const RUNTIME_BRIDGE_KEY_STATUS: &str = "status";
const RUNTIME_BRIDGE_KEY_UNREAD: &str = "unread";
const RUNTIME_BRIDGE_KEY_MARKED: &str = "marked";
const RUNTIME_BRIDGE_KEY_IS_BOOKMARKED: &str = "isBookmarked";
const RUNTIME_BRIDGE_KEY_VISIBLE: &str = "visible";
const RUNTIME_BRIDGE_KEY_CONTAINER_WIDTH: &str = "containerWidth";
const RUNTIME_BRIDGE_KEY_CONTAINER_HEIGHT: &str = "containerHeight";
const RUNTIME_EVENT_TAB_ENTERED_FULLSCREEN: &str = "onTabEnteredFullscreen";
const RUNTIME_EVENT_TAB_EXITED_FULLSCREEN: &str = "onTabExitedFullscreen";
const RUNTIME_EVENT_TAB_BOOKMARK_CHANGED: &str = "onTabBookmarkChanged";
const RUNTIME_EVENT_EXTENSION_PANEL_VISIBLE_CHANGE: &str = "onExtensionPanelVisibleChange";
const RUNTIME_EVENT_CONTAINER_SIZE_CHANGE: &str = "onContainerSizeChange";
const RUNTIME_EVENT_SEARCH_INPUT_CHANGED: &str = "onSearchInputChanged";
const RUNTIME_AUTH_CONTEXT_KEY_CLIENT_ID: &str = "clientId";
const RUNTIME_AUTH_CONTEXT_KEY_APP_ID: &str = "appId";
const RUNTIME_AUTH_CONTEXT_KEY_TAB_ID: &str = RUNTIME_BRIDGE_KEY_TAB_ID;
const RUNTIME_AUTH_CONTEXT_KEY_APP_URL: &str = RUNTIME_BRIDGE_KEY_APP_URL;
const RUNTIME_LOGIN_RESULT_EVENT_NAME: &str = "onLoginResult";
const RUNTIME_LOGIN_RESULT_KEY_TYPE: &str = "type";
const RUNTIME_LOGIN_RESULT_KEY_CODE: &str = "code";
const RUNTIME_LOGIN_RESULT_KEY_ID_TOKEN: &str = "id_token";
const RUNTIME_LOGIN_RESULT_KEY_ERROR: &str = "error";
const RUNTIME_LOGIN_RESULT_KEY_MESSAGE: &str = "message";
const RUNTIME_LOGIN_TYPE_WECHAT: &str = "wechat";
const RUNTIME_LOGIN_TYPE_APPLE: &str = "apple";
const RUNTIME_LOGIN_TYPE_GOOGLE: &str = "google";
const RUNTIME_TAB_PROPS_KEY_UNIFIED_SCRIPT: &str = "unifiedScript";
const RUNTIME_TAB_PROPS_KEY_RIGHT_PANEL_DEFAULT_VISIBLE: &str = "rightPanelDefaultVisible";
const RUNTIME_TAB_PROPS_KEY_ID: &str = "id";
const RUNTIME_TAB_PROPS_KEY_BACKGROUND_COLOR: &str = "backgroundColor";
const RUNTIME_TAB_PROPS_KEY_RELOAD_KEY: &str = "reloadKey";
const RUNTIME_TAB_PROPS_KEY_HTML_CONTENT: &str = "htmlContent";
const RUNTIME_ROOT_PROPS_KEY_TAB: &str = "tab";
const RUNTIME_ROOT_PROPS_KEY_TABS: &str = "tabs";
const RUNTIME_ROOT_PROPS_KEY_SINGLE_TAB_MODE: &str = "singleTabMode";
const RUNTIME_ROOT_PROPS_KEY_CURRENT_TAB_ID: &str = "currentTabId";
const RUNTIME_ROOT_PROPS_KEY_IS_LEFT_PANEL_VISIBLE: &str = "isLeftPanelVisible";
const RUNTIME_ROOT_PROPS_KEY_IS_RIGHT_PANEL_VISIBLE: &str = "isRightPanelVisible";

pub fn actr_bridge_contract() -> ActrBridgeContractInfo {
    use bridge::actr_bridge_contract as contract;
    use bridge::actr_bridge_contract::key;

    ActrBridgeContractInfo {
        module_name: contract::MODULE_NAME.to_string(),
        stream_event_name: contract::STREAM_EVENT_NAME.to_string(),
        status_connected: contract::STATUS_CONNECTED.to_string(),
        stream_type_text: contract::STREAM_TYPE_TEXT.to_string(),
        stream_type_voice: contract::STREAM_TYPE_VOICE.to_string(),
        stream_type_attachment_result: contract::STREAM_TYPE_ATTACHMENT_RESULT.to_string(),
        stream_status_ok: contract::STREAM_STATUS_OK,
        stream_status_error: contract::STREAM_STATUS_ERROR,
        stream_metadata_eos_key: contract::STREAM_METADATA_EOS_KEY.to_string(),
        stream_metadata_aos_key: contract::STREAM_METADATA_AOS_KEY.to_string(),
        stream_metadata_terminal_value: contract::STREAM_METADATA_TERMINAL_VALUE.to_string(),
        attachment_result_timeout_ms: contract::ATTACHMENT_RESULT_TIMEOUT_MS,
        attachment_chunk_size_bytes: contract::ATTACHMENT_CHUNK_SIZE_BYTES,
        attachment_checksum_algorithm_md5: contract::ATTACHMENT_CHECKSUM_ALGORITHM_MD5.to_string(),
        attachment_status_code_ok: contract::ATTACHMENT_STATUS_CODE_OK,
        attachment_type_unknown: contract::ATTACHMENT_TYPE_UNKNOWN,
        attachment_type_image: contract::ATTACHMENT_TYPE_IMAGE,
        attachment_type_document: contract::ATTACHMENT_TYPE_DOCUMENT,
        attachment_type_audio: contract::ATTACHMENT_TYPE_AUDIO,
        attachment_type_video: contract::ATTACHMENT_TYPE_VIDEO,
        attachment_type_other: contract::ATTACHMENT_TYPE_OTHER,
        attachment_upload_status_unknown: contract::ATTACHMENT_UPLOAD_STATUS_UNKNOWN,
        attachment_upload_status_completed: contract::ATTACHMENT_UPLOAD_STATUS_COMPLETED,
        attachment_upload_status_failed: contract::ATTACHMENT_UPLOAD_STATUS_FAILED,
        ask_service_prompt_route_key: contract::ASK_SERVICE_PROMPT_ROUTE_KEY.to_string(),
        ask_service_attach_route_key: contract::ASK_SERVICE_ATTACH_ROUTE_KEY.to_string(),
        ask_service_prepare_attachment_upload_route_key:
            contract::ASK_SERVICE_PREPARE_ATTACHMENT_UPLOAD_ROUTE_KEY.to_string(),
        ask_local_service_send_text_route_key: contract::ASK_LOCAL_SERVICE_SEND_TEXT_ROUTE_KEY
            .to_string(),
        ask_local_service_start_send_voice_route_key:
            contract::ASK_LOCAL_SERVICE_START_SEND_VOICE_ROUTE_KEY.to_string(),
        ask_local_service_send_voice_chunk_route_key:
            contract::ASK_LOCAL_SERVICE_SEND_VOICE_CHUNK_ROUTE_KEY.to_string(),
        ios_ask_service_prompt_timeout_ms: contract::IOS_ASK_SERVICE_PROMPT_TIMEOUT_MS,
        ios_ask_service_prepare_attachment_upload_timeout_ms:
            contract::IOS_ASK_SERVICE_PREPARE_ATTACHMENT_UPLOAD_TIMEOUT_MS,
        android_ask_service_prompt_timeout_ms: contract::ANDROID_ASK_SERVICE_PROMPT_TIMEOUT_MS,
        android_ask_service_attach_timeout_ms: contract::ANDROID_ASK_SERVICE_ATTACH_TIMEOUT_MS,
        android_ask_local_service_rpc_timeout_ms:
            contract::ANDROID_ASK_LOCAL_SERVICE_RPC_TIMEOUT_MS,
        error_invalid_input: contract::ERROR_INVALID_INPUT.to_string(),
        error_unknown: contract::ERROR_UNKNOWN.to_string(),
        error_prompt_request: contract::ERROR_PROMPT_REQUEST.to_string(),
        error_not_connected: contract::ERROR_NOT_CONNECTED.to_string(),
        error_actr: contract::ERROR_ACTR.to_string(),
        error_generic: contract::ERROR_GENERIC.to_string(),
        key_tab_id: key::TAB_ID.to_string(),
        key_actor_type: key::ACTOR_TYPE.to_string(),
        key_path: key::PATH.to_string(),
        key_status: key::STATUS.to_string(),
        key_message_id: key::MESSAGE_ID.to_string(),
        key_session_id: key::SESSION_ID.to_string(),
        key_text: key::TEXT.to_string(),
        key_message: key::MESSAGE.to_string(),
        key_stream_id: key::STREAM_ID.to_string(),
        key_stream_id_upper: key::STREAM_ID_UPPER.to_string(),
        key_text_response_stream_id: key::TEXT_RESPONSE_STREAM_ID.to_string(),
        key_voice_response_stream_id: key::VOICE_RESPONSE_STREAM_ID.to_string(),
        key_attachment_ids: key::ATTACHMENT_IDS.to_string(),
        key_location: key::LOCATION.to_string(),
        key_latitude: key::LATITUDE.to_string(),
        key_longitude: key::LONGITUDE.to_string(),
        key_address: key::ADDRESS.to_string(),
        key_place_name: key::PLACE_NAME.to_string(),
        key_file_data: key::FILE_DATA.to_string(),
        key_attachment_id: key::ATTACHMENT_ID.to_string(),
        key_checksum: key::CHECKSUM.to_string(),
        key_checksum_algorithm: key::CHECKSUM_ALGORITHM.to_string(),
        key_size_bytes: key::SIZE_BYTES.to_string(),
        key_result_stream_id: key::RESULT_STREAM_ID.to_string(),
        key_id: key::ID.to_string(),
        key_filename: key::FILENAME.to_string(),
        key_type: key::TYPE.to_string(),
        key_status_code: key::STATUS_CODE.to_string(),
        key_error_message: key::ERROR_MESSAGE.to_string(),
        key_question_id: key::QUESTION_ID.to_string(),
        key_chunk_id: key::CHUNK_ID.to_string(),
        key_metadata: key::METADATA.to_string(),
        key_metadata_key: key::METADATA_KEY.to_string(),
        key_metadata_value: key::METADATA_VALUE.to_string(),
        key_sequence: key::SEQUENCE.to_string(),
        key_timestamp_ms: key::TIMESTAMP_MS.to_string(),
    }
}

pub fn parse_actr_actor_type(raw: String) -> Result<ActrActorTypeInfo, CoreError> {
    bridge::parse_actr_actor_type(&raw)
        .map(|actor_type| ActrActorTypeInfo {
            publisher: actor_type.publisher,
            name: actor_type.name,
            version: actor_type.version,
        })
        .ok_or(CoreError::InvalidActrActorType(raw))
}

pub fn asr_model_language_dir(language: String) -> String {
    speech::asr_model_language_dir(&language).to_string()
}

pub fn asr_model_plan(language: String) -> AsrModelPlanInfo {
    let plan = speech::asr_model_plan(&language);
    AsrModelPlanInfo {
        language_dir: plan.language_dir,
        asset_path: plan.asset_path,
        storage_relative_path: plan.storage_relative_path,
        ready_marker: plan.ready_marker,
    }
}

pub fn asr_bridge_contract() -> AsrBridgeContractInfo {
    use speech::asr_bridge_contract as contract;

    AsrBridgeContractInfo {
        module_name: contract::MODULE_NAME.to_string(),
        result_event_name: contract::RESULT_EVENT_NAME.to_string(),
        error_event_name: contract::ERROR_EVENT_NAME.to_string(),
        error_init: contract::ERROR_INIT.to_string(),
        error_feed: contract::ERROR_FEED.to_string(),
        error_stop: contract::ERROR_STOP.to_string(),
        key_text: contract::key::TEXT.to_string(),
        key_is_final: contract::key::IS_FINAL.to_string(),
    }
}

pub fn tts_bridge_contract() -> TtsBridgeContractInfo {
    use speech::tts_bridge_contract as contract;
    use speech::tts_bridge_contract::key;

    TtsBridgeContractInfo {
        module_name: contract::MODULE_NAME.to_string(),
        event_start: contract::EVENT_START.to_string(),
        event_pause: contract::EVENT_PAUSE.to_string(),
        event_resume: contract::EVENT_RESUME.to_string(),
        event_finish: contract::EVENT_FINISH.to_string(),
        event_error: contract::EVENT_ERROR.to_string(),
        key_rate: key::RATE.to_string(),
        key_pitch_multiplier: key::PITCH_MULTIPLIER.to_string(),
        key_volume: key::VOLUME.to_string(),
        key_language: key::LANGUAGE.to_string(),
        key_voice_identifier: key::VOICE_IDENTIFIER.to_string(),
        key_cancelled: key::CANCELLED.to_string(),
        key_message: key::MESSAGE.to_string(),
    }
}

pub fn email_bridge_contract() -> EmailBridgeContractInfo {
    use bridge::email_bridge_contract as contract;

    EmailBridgeContractInfo {
        module_name: contract::MODULE_NAME.to_string(),
        method_compose_email: contract::METHOD_COMPOSE_EMAIL.to_string(),
        uri_scheme_mailto: contract::URI_SCHEME_MAILTO.to_string(),
        error_invalid_recipients: contract::ERROR_INVALID_RECIPIENTS.to_string(),
        error_open_failed: contract::ERROR_OPEN_FAILED.to_string(),
    }
}

pub fn user_info_bridge_contract() -> UserInfoBridgeContractInfo {
    use bridge::user_info_bridge_contract as contract;
    use bridge::user_info_bridge_contract::key;

    UserInfoBridgeContractInfo {
        module_name: contract::MODULE_NAME.to_string(),
        method_get_current_user_info: contract::METHOD_GET_CURRENT_USER_INFO.to_string(),
        method_login_with_type: contract::METHOD_LOGIN_WITH_TYPE.to_string(),
        event_user_info_changed: contract::EVENT_USER_INFO_CHANGED.to_string(),
        event_login_status_changed: contract::EVENT_LOGIN_STATUS_CHANGED.to_string(),
        event_user_logout: contract::EVENT_USER_LOGOUT.to_string(),
        key_id: key::ID.to_string(),
        key_login_api_url: key::LOGIN_API_URL.to_string(),
        key_user_info: key::USER_INFO.to_string(),
        key_user_identity: key::USER_IDENTITY.to_string(),
        key_session_token: key::SESSION_TOKEN.to_string(),
        key_created_at: key::CREATED_AT.to_string(),
        key_updated_at: key::UPDATED_AT.to_string(),
        key_request_context: key::REQUEST_CONTEXT.to_string(),
        key_is_logged_in: key::IS_LOGGED_IN.to_string(),
    }
}

pub fn parse_vosk_result_text(json: String) -> String {
    speech::parse_vosk_result_text(&json)
}

pub fn parse_vosk_partial_text(json: String) -> String {
    speech::parse_vosk_partial_text(&json)
}

pub fn post_process_zh_asr_segments(text: String, segments: Vec<AsrSegmentInfo>) -> String {
    let segments = segments
        .into_iter()
        .map(|segment| speech::AsrSegment {
            word: segment.word,
            nature: segment.nature,
        })
        .collect::<Vec<_>>();

    speech::post_process_zh_asr_segments(&text, &segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_manifest_info_flattens_host_fields() {
        let info = parse_actr_package_manifest(
            r##"
                format = "actr-package/2"
                [actor]
                publisher = "demo"
                name = "app"
                version = "1.0.0"
                [workload]
                profile = "keel.guest/1"
                entry = "workload/guest.js"
                [workload.keel]
                runtime = "js"
                guest-api = "keel.guest/1"
                [host]
                profile = "actr.host/1"
                api = "^1"
                required-capabilities = ["ui.render"]
                [extensions.actrium]
                title = "Demo"
                theme-color = "#111111"
                [signature]
                algorithm = "ed25519"
                key-id = "publisher-test"
                [[files]]
                path = "workload/guest.js"
                role = "workload"
                media-type = "text/javascript"
                size = 1
                sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            "##
            .to_string(),
        )
        .unwrap();

        assert_eq!(info.app_id.as_deref(), Some("demo/app@1.0.0"));
        assert_eq!(info.theme_color.as_deref(), Some("#111111"));
        assert_eq!(info.contract_name.as_deref(), Some("keel.guest"));
        assert_eq!(info.contract_version, Some(1));
        assert_eq!(info.system_app_type, None);
    }

    #[test]
    fn ffi_contract_gate_accepts_absent_contract() {
        let gate = evaluate_contract(None, 0);
        assert_eq!(gate.status, ContractGateStatus::Compat);
        assert_eq!(gate.blocked_reason, None);
    }

    #[test]
    fn ffi_system_app_info_is_canonical() {
        let info = system_app_info("globalSearch".to_string()).unwrap();
        assert_eq!(info.kind, "globalSearch");
        assert_eq!(info.title, "Search");
        assert_eq!(info.about_url, "about:globalSearch");
    }

    #[test]
    fn ffi_system_app_type_helpers_match_runtime_strings() {
        assert_eq!(system_app_type_bookmarks(), "bookmarks");
        assert_eq!(system_app_type_settings(), "settings");
        assert_eq!(system_app_type_global_search(), "globalSearch");
    }

    #[test]
    fn ffi_app_source_type_helpers_match_apple_repository_contract() {
        assert_eq!(app_source_type_downloaded(), "downloaded");
        assert_eq!(app_source_type_local_dev(), "localDev");
        assert_eq!(
            parse_app_source_type("downloaded".to_string()).as_deref(),
            Some("downloaded")
        );
        assert_eq!(
            parse_app_source_type("localDev".to_string()).as_deref(),
            Some("localDev")
        );
        assert_eq!(parse_app_source_type("local-dev".to_string()), None);

        let info = app_source_type_info("localDev".to_string()).unwrap();
        assert_eq!(info.kind, "localDev");
        assert_eq!(info.display_name, "Local Development");
    }

    #[test]
    fn ffi_browser_search_engine_contract_is_shared() {
        assert_eq!(browser_search_engine_default_id(), "google");
        assert_eq!(
            browser_search_engine_normalize_id(Some(" DuckDuckGo ".to_string())),
            "duckduckgo"
        );
        assert_eq!(browser_search_engine_normalize_id(None), "google");
        assert_eq!(
            browser_search_engine_normalize_id(Some("missing".to_string())),
            "google"
        );

        let engines = browser_search_engines();
        assert_eq!(engines.len(), 3);
        assert_eq!(engines[0].id, "google");
        assert_eq!(engines[0].display_name, "Google");
        assert_eq!(
            engines[0].search_url_prefix,
            "https://www.google.com/search?q="
        );

        let bing = browser_search_engine_info(Some("bing".to_string()));
        assert_eq!(bing.display_name, "Bing");
        assert_eq!(
            browser_search_url(
                "actrium browser".to_string(),
                Some("duckduckgo".to_string())
            ),
            "https://duckduckgo.com/?q=actrium%20browser"
        );
        assert_eq!(
            browser_search_url("a&b=c".to_string(), Some("google".to_string())),
            "https://www.google.com/search?q=a%26b%3Dc"
        );
        assert_eq!(
            browser_search_url("\u{4f60}\u{597d}".to_string(), Some("google".to_string())),
            "https://www.google.com/search?q=%E4%BD%A0%E5%A5%BD"
        );
    }

    #[test]
    fn ffi_browser_data_snapshot_contract_is_shared() {
        let contract = browser_data_contract();
        assert_eq!(contract.schema_version, 1);
        assert_eq!(contract.max_history_items, 500);
        assert_eq!(contract.max_bookmark_items, 500);
        assert_eq!(contract.max_session_tabs, 50);
        assert_eq!(contract.max_title_length, 160);
        assert_eq!(contract.home_address, "actro://home");
        assert_eq!(contract.history_address, "actro://history");
        assert_eq!(contract.bookmarks_address, "actro://bookmarks");

        let history = browser_history_snapshot_normalize_json(
            r#"{"version":99,"items":[{"title":"","address":"https://example.com/docs","visitedAtUnixSeconds":-1,"visitCount":0}]}"#.to_string(),
        );
        assert!(history.contains(r#""version":1"#));
        assert!(history.contains(r#""title":"example.com"#));
        assert!(history.contains(r#""visitedAtUnixSeconds":0"#));
        assert!(history.contains(r#""visitCount":1"#));

        let session = browser_session_snapshot_normalize_json(
            r#"{"version":99,"activeTabIndex":9,"tabs":[{"title":"","address":"ACTRO://BOOKMARKS"}]}"#.to_string(),
        );
        assert!(session.contains(r#""activeTabIndex":0"#));
        assert!(session.contains(r#""title":"New Tab"#));
        assert!(session.contains(r#""address":"actro://bookmarks"#));
        assert_eq!(
            browser_session_snapshot_normalize_json(
                r#"{"version":1,"activeTabIndex":0,"tabs":[]}"#.to_string()
            ),
            "null"
        );
        assert!(browser_session_home_snapshot_json().contains(r#""address":"actro://home"#));
    }

    #[test]
    fn ffi_browser_storage_schema_contract_is_shared() {
        let schema = browser_storage_schema();
        assert_eq!(schema.schema_version, 6);
        assert_eq!(schema.database_name, "actro_browser.db");
        assert!(
            schema
                .table_names
                .contains(&"browser_history_pages".to_string())
        );
        assert!(
            schema
                .table_names
                .contains(&"browser_session_tabs".to_string())
        );
        assert!(
            schema
                .statements
                .iter()
                .any(|statement| statement.contains("browser_bookmarks"))
        );
        assert_eq!(browser_storage_schema_version(), 6);
        assert_eq!(browser_storage_database_name(), "actro_browser.db");
        assert!(
            browser_storage_schema_statements()
                .iter()
                .any(|statement| statement.contains("browser_storage_schema_version"))
        );

        let migration_plan = browser_storage_schema_migration_plan_json();
        assert!(migration_plan.contains("\"schemaVersion\":6"));
        assert!(migration_plan.contains("\"addColumns\""));
        assert!(migration_plan.contains("\"idempotentStatements\""));

        let sql = browser_storage_repository_sql();
        assert!(
            sql.upsert_history_page
                .contains("INSERT INTO browser_history_pages")
        );
        assert!(sql.upsert_bookmark.contains("ON CONFLICT(address)"));
        assert_eq!(
            sql.select_session,
            "SELECT active_tab_index, owner_kind, owner_id, surface_kind, surface_id FROM browser_sessions WHERE id = ?"
        );
        assert_eq!(
            sql.delete_session_tabs,
            "DELETE FROM browser_session_tabs WHERE session_id = ?"
        );

        let sync_sql = browser_storage_sync_sql();
        assert!(
            sync_sql
                .select_dirty_history_pages
                .contains("browser_history_pages")
        );
        assert!(
            sync_sql
                .apply_remote_history_page
                .contains("ON CONFLICT(address)")
        );
        assert!(sync_sql.upsert_sync_cursor.contains("browser_sync_cursors"));
        assert!(
            sync_sql
                .mark_session_tab_synced
                .contains("sync_revision <= ?")
        );

        let data_model = browser_storage_data_model_json();
        assert!(data_model.contains(r#""storageSchemaVersion":6"#));
        assert!(data_model.contains(r#""recordType":"historyPage""#));
        assert!(data_model.contains(r#""recordType":"sessionTab""#));
        assert!(data_model.contains(r#""localSyncCursorTable":"browser_sync_cursors""#));

        let conflict_policy = browser_storage_sync_conflict_policy_json();
        assert!(conflict_policy.contains(r#""policy":"skipLocalDirtyOrNewerRevision""#));
        assert!(conflict_policy.contains(r#""reason":"localDirtyOrNewerRevision""#));
        assert!(conflict_policy.contains(r#""appliesToRecordTypes""#));

        let merge_policy = browser_storage_sync_merge_policy_json();
        assert!(merge_policy.contains(r#""mergePolicy":"historyPageAggregateByAddress""#));
        assert!(merge_policy.contains(r#""collection":"bookmarks""#));
        assert!(merge_policy.contains(r#""identity":"stable session id plus stable tab id"#));

        let address_policy = browser_storage_address_canonicalization_policy_json();
        assert!(address_policy.contains(r#""policy":"browserStorageAddressCanonicalization""#));
        assert!(address_policy.contains(r#""acceptedExternalSchemes""#));

        let canonical_addresses = browser_storage_canonicalize_addresses_json(
            r#"{"addresses":[" HTTPS://Example.COM:443/docs ","example.com"]}"#.to_string(),
        );
        assert!(canonical_addresses.contains(r#""canonicalAddress":"https://example.com/docs""#));
        assert!(canonical_addresses.contains(r#""reason":"invalidUrl""#));

        let folder_tree_policy = browser_storage_bookmark_folder_tree_integrity_policy_json();
        assert!(
            folder_tree_policy.contains(r#""policy":"browserStorageBookmarkFolderTreeIntegrity""#)
        );
        assert!(folder_tree_policy.contains(r#""integrityRules""#));

        let folder_tree_plan = browser_storage_bookmark_folder_tree_integrity_plan_json(
            r#"{"folders":[{"id":"a","parentId":"b"},{"id":"b","parentId":"a"}]}"#.to_string(),
        );
        assert!(folder_tree_plan.contains(r#""classification":"cycleDetected""#));
        assert!(folder_tree_plan.contains(r#""action":"rejectFolderMove""#));

        let bookmark_restore_policy = browser_storage_bookmark_restore_placement_policy_json();
        assert!(
            bookmark_restore_policy
                .contains(r#""policy":"browserStorageBookmarkRestorePlacement""#)
        );
        assert!(bookmark_restore_policy.contains(r#""placementRules""#));

        let bookmark_restore_plan = browser_storage_bookmark_restore_placement_plan_json(
            r#"{"folders":[{"id":"old","deletedAtUnixSeconds":1}],"bookmarks":[{"id":"bookmark-a","folderId":"old","title":"A","address":"https://example.com/a"}]}"#.to_string(),
        );
        assert!(bookmark_restore_plan.contains(r#""classification":"reassigned""#));
        assert!(
            bookmark_restore_plan.contains(r#""placementAction":"movedToRootAfterFolderDeletion""#)
        );

        let bookmark_ownership_policy = browser_storage_bookmark_ownership_policy_json();
        assert!(
            bookmark_ownership_policy.contains(r#""policy":"browserStorageBookmarkOwnership""#)
        );
        assert!(bookmark_ownership_policy.contains(r#""ownershipRules""#));

        let bookmark_ownership_plan = browser_storage_bookmark_ownership_plan_json(
            r#"{"bookmarks":[{"id":"user-bookmark","title":"User","address":"https://example.com/a","ownerKind":"user","createdAtUnixSeconds":1,"updatedAtUnixSeconds":2},{"id":"tab-bookmark","title":"Tab","address":"https://example.com/a","ownerKind":"tab","ownerId":"tab-a","createdAtUnixSeconds":1,"updatedAtUnixSeconds":3}]}"#.to_string(),
        );
        assert!(bookmark_ownership_plan.contains(r#""classification":"deduplicated""#));
        assert!(bookmark_ownership_plan.contains(r#""ownershipAction":"retainUserOwned""#));

        let history_visit_policy = browser_storage_history_visit_aggregation_policy_json();
        assert!(
            history_visit_policy.contains(r#""policy":"browserStorageHistoryVisitAggregation""#)
        );
        assert!(history_visit_policy.contains(r#""aggregationRules""#));

        let history_visit_plan = browser_storage_history_visit_aggregation_plan_json(
            r#"{"visits":[{"id":"visit-1","address":"HTTP://Example.COM:80/docs","title":"One","visitedAtUnixSeconds":100},{"id":"visit-2","address":"http://example.com/docs","title":"Two","visitedAtUnixSeconds":200}]}"#.to_string(),
        );
        assert!(history_visit_plan.contains(r#""classification":"aggregated""#));
        assert!(history_visit_plan.contains(r#""visitCount":2"#));

        let session_tab_ordering_policy = browser_storage_session_tab_ordering_policy_json();
        assert!(
            session_tab_ordering_policy.contains(r#""policy":"browserStorageSessionTabOrdering""#)
        );
        assert!(session_tab_ordering_policy.contains(r#""orderingRules""#));

        let session_tab_ordering_plan = browser_storage_session_tab_ordering_plan_json(
            r#"{"sessionTabs":[{"sessionId":"session-a","tabId":"tab-b","tabIndex":2,"title":"B","address":"https://example.com/b"},{"sessionId":"session-a","tabId":"tab-a","tabIndex":1,"title":"A","address":"https://example.com/a"}]}"#.to_string(),
        );
        assert!(session_tab_ordering_plan.contains(r#""classification":"ordered""#));
        assert!(session_tab_ordering_plan.contains(r#""orderedTabIds":["tab-a","tab-b"]"#));

        let session_tab_id_plan = browser_storage_session_tab_id_plan_json(
            r#"[{"id":"","index":0,"address":"actro://home"},{"id":"tab-a","index":1,"address":"https://example.com/a"}]"#.to_string(),
        );
        assert!(session_tab_id_plan.contains(r#""policy":"browserStorageSessionTabId""#));
        assert!(
            session_tab_id_plan
                .contains(r#""tabIds":["fa3d824d-efbc-593f-b327-a463b5e0973c","tab-a"]"#)
        );

        let active_tab_policy = browser_storage_session_active_tab_consistency_policy_json();
        assert!(
            active_tab_policy.contains(r#""policy":"browserStorageSessionActiveTabConsistency""#)
        );
        assert!(active_tab_policy.contains(r#""consistencyRules""#));

        let active_tab_plan = browser_storage_session_active_tab_consistency_plan_json(
            r#"{"sessions":[{"id":"session-a","activeTabIndex":9}],"sessionTabs":[{"sessionId":"session-a","tabId":"tab-a","tabIndex":0,"title":"A","address":"https://example.com/a"}]}"#.to_string(),
        );
        assert!(active_tab_plan.contains(r#""classification":"adjusted""#));
        assert!(active_tab_plan.contains(r#""activeTabId":"tab-a""#));

        let retention_policy = browser_storage_retention_pruning_policy_json();
        assert!(retention_policy.contains(r#""policy":"browserStorageRetentionPruning""#));
        assert!(retention_policy.contains(r#""maxHistoryPageCount""#));

        let retention_plan = browser_storage_retention_pruning_plan_json(
            r#"{"maxHistoryPages":1,"historyPages":[{"address":"https://example.com/a","lastVisitedAtUnixSeconds":1},{"address":"https://example.com/b","lastVisitedAtUnixSeconds":2}],"sessionTabs":[{"sessionId":"session-a","tabId":"tab-a","tabIndex":0,"title":"A","address":"https://example.com/a"},{"sessionId":"session-a","tabId":"tab-b","tabIndex":1,"title":"B","address":"https://example.com/b"}],"maxSessionTabsPerSession":1}"#.to_string(),
        );
        assert!(retention_plan.contains(r#""classification":"pruneHistoryAndSessions""#));
        assert!(retention_plan.contains(r#""reason":"historyPageLimitExceeded""#));
        assert!(retention_plan.contains(r#""reason":"sessionTabLimitExceeded""#));

        let empty_batch = empty_browser_storage_sync_batch_json("".to_string(), "".to_string(), -1);
        assert!(empty_batch.contains(r#""version":1"#));
        assert!(empty_batch.contains(r#""storageSchemaVersion":6"#));

        let normalized_batch = normalize_browser_storage_sync_batch_json(
            r#"{"version":99,"storageSchemaVersion":6,"scope":"profile","deviceId":"device-a","generatedAtUnixSeconds":1,"records":[]}"#.to_string(),
        );
        assert!(normalized_batch.contains(r#""scope":"profile"#));
        assert!(normalized_batch.contains(r#""records":[]"#));

        let dirty_rows = r#"[{"record_type":"historyPage","record_id":"https://example.com","sync_revision":1,"sync_state":"local","origin_device_id":"","updated_at":1,"address":"https://example.com","title":"","visit_count":1,"last_visited_at":1,"created_at":1}]"#.to_string();
        let dirty_batch = browser_storage_sync_batch_from_dirty_rows_json(
            "profile".to_string(),
            "device-a".to_string(),
            2,
            dirty_rows.clone(),
        );
        assert!(dirty_batch.contains(r#""recordType":"historyPage""#));
        assert!(dirty_batch.contains(r#""title":"example.com""#));

        let dirty_push_request = browser_storage_sync_push_request_from_dirty_rows_json(
            "profile".to_string(),
            "device-a".to_string(),
            2,
            dirty_rows,
        );
        assert!(dirty_push_request.contains(r#""requestType":"push""#));
        assert!(dirty_push_request.contains(r#""pushBatch""#));

        let normalized_ack = normalize_browser_storage_sync_ack_batch_json(
            r#"{"version":99,"storageSchemaVersion":6,"scope":"profile","deviceId":"device-a","syncedAtUnixSeconds":1,"records":[],"cursors":[{"collection":"historyPages","cursor":"cursor-a"}]}"#.to_string(),
        );
        assert!(normalized_ack.contains(r#""storageSchemaVersion":6"#));
        assert!(normalized_ack.contains(r#""lastSyncedAtUnixSeconds":1"#));

        let normalized_cursors = normalize_browser_storage_sync_cursors_json(
            r#"[{"collection":"historyPages","cursor":"cursor-a"},{"collection":"bad","cursor":"ignored"}]"#.to_string(),
        );
        assert!(normalized_cursors.contains(r#""collection":"historyPages""#));
        assert!(!normalized_cursors.contains("ignored"));

        let push_request = browser_storage_sync_push_request_json(normalized_batch.clone());
        assert!(push_request.contains(r#""requestType":"push""#));
        assert!(push_request.contains(r#""pushBatch""#));

        let pull_request = browser_storage_sync_pull_request_json(
            "profile".to_string(),
            "device-a".to_string(),
            2,
            normalized_cursors,
            0,
        );
        assert!(pull_request.contains(r#""requestType":"pull""#));
        assert!(pull_request.contains(r#""maxRecordsPerCollection":200"#));

        let ack_plan = browser_storage_sync_ack_plan_json(
            r#"{"version":1,"storageSchemaVersion":6,"scope":"profile","deviceId":"device-a","syncedAtUnixSeconds":1,"records":[{"recordType":"historyPage","recordId":"https://example.com","syncRevision":1}],"cursors":[]}"#.to_string(),
        );
        assert!(ack_plan.contains(r#""statementKey":"markHistoryPageSynced""#));
        assert!(ack_plan.contains(r#""params":[1,1,"device-a","https://example.com/",1]"#));

        let remote_apply_plan = browser_storage_sync_remote_apply_plan_json(
            r#"{"version":1,"storageSchemaVersion":6,"scope":"profile","deviceId":"device-b","generatedAtUnixSeconds":2,"records":[{"recordId":"ignored","syncRevision":3,"syncState":"remote","originDeviceId":"device-b","updatedAtUnixSeconds":2,"recordType":"historyPage","payload":{"address":"https://example.com","title":"Example","visitCount":1,"lastVisitedAtUnixSeconds":2,"createdAtUnixSeconds":1,"updatedAtUnixSeconds":2}}]}"#.to_string(),
        );
        assert!(remote_apply_plan.contains(r#""statementKey":"applyRemoteHistoryPage""#));
        assert!(remote_apply_plan.contains(r#""conflictPolicy":"skipLocalDirtyOrNewerRevision""#));

        let remote_apply_result = browser_storage_sync_remote_apply_result_json(
            remote_apply_plan,
            r#"{"version":1,"storageSchemaVersion":6,"scope":"profile","deviceId":"device-b","generatedAtUnixSeconds":3,"results":[{"operationIndex":0,"rowsAffected":0}]}"#.to_string(),
        );
        assert!(remote_apply_result.contains(r#""skippedCount":1"#));
        assert!(remote_apply_result.contains(r#""reason":"localDirtyOrNewerRevision""#));

        let server_merge_plan =
            browser_storage_sync_server_merge_plan_json(dirty_push_request.clone());
        assert!(server_merge_plan.contains(r#""statementKey":"mergePushedHistoryPage""#));
        assert!(server_merge_plan.contains(r#""mergePolicy":"historyPageAggregateByAddress""#));

        let server_merge_result = browser_storage_sync_server_merge_result_json(
            server_merge_plan,
            r#"{"version":1,"storageSchemaVersion":6,"scope":"profile","deviceId":"device-a","generatedAtUnixSeconds":5,"results":[{"operationIndex":0,"rowsAffected":1}]}"#.to_string(),
        );
        assert!(server_merge_result.contains(r#""acceptedCount":1"#));
        assert!(server_merge_result.contains(r#""ackBatch""#));

        let empty_profile_batch =
            empty_browser_storage_sync_batch_json("profile".to_string(), "device-a".to_string(), 2);
        let cycle_response =
            browser_storage_sync_cycle_response_json(empty_profile_batch, normalized_ack);
        assert!(cycle_response.contains(r#""responseType":"syncCycle""#));
        assert!(cycle_response.contains(r#""remoteBatch""#));
        assert!(cycle_response.contains(r#""ackBatch""#));

        let normalized_cycle_response =
            normalize_browser_storage_sync_cycle_response_json(cycle_response.clone());
        assert!(normalized_cycle_response.contains(r#""responseType":"syncCycle""#));

        let client_cycle_plan =
            browser_storage_sync_client_cycle_plan_json(normalized_cycle_response.clone());
        assert!(client_cycle_plan.contains(r#""action":"applyAck""#));
        assert!(client_cycle_plan.contains(r#""ackPlan""#));

        let transport_contract = browser_storage_sync_transport_contract_json();
        assert!(transport_contract.contains("/v1/browser-storage/sync/cycle"));
        assert!(transport_contract.contains(r#""Authorization""#));

        let cloud_store_contract = browser_storage_sync_cloud_store_contract_json();
        assert!(cloud_store_contract.contains(r#""rootPath":"Actro/BrowserSync""#));
        assert!(cloud_store_contract.contains(r#""operation":"write""#));
        assert!(cloud_store_contract.contains("expectedRevision"));
        assert!(cloud_store_contract.contains(r#""adapterKind":"opendal""#));

        let provider_adapter_contract = browser_storage_sync_cloud_provider_adapter_contract_json();
        assert!(provider_adapter_contract.contains(r#""planType":"cloudProviderAdapterPlan""#));
        assert!(provider_adapter_contract.contains("coarse rounds"));

        let provider_adapter_plan = browser_storage_sync_cloud_provider_adapter_plan_json(
            r#"{"providerKind":"localFolder","rootPath":"Actro/BrowserSync","revisionKind":"contentHash","supportsPrefixList":true,"supportsPagedList":true,"supportsConditionalWrite":true,"supportsConditionalDelete":true,"supportsReadIfChanged":true,"supportsAtomicRename":false,"supportsAppFolder":true,"backgroundSync":"platformScheduled","conflictResolution":"optimisticRevision","maxObjectBytes":8388608,"maxPageSize":1000}"#.to_string(),
            8,
        );
        assert!(provider_adapter_plan.contains(r#""classification":"ready""#));
        assert!(provider_adapter_plan.contains(r#""productionReady":true"#));

        let provider_connection_contract =
            browser_storage_sync_cloud_provider_connection_contract_json();
        assert!(
            provider_connection_contract
                .contains(r#""planType":"cloudProviderConnectionStatusPolicy""#)
        );
        assert!(provider_connection_contract.contains("No Actrium product server"));

        let provider_error_policy = browser_storage_sync_cloud_provider_error_policy_json();
        assert!(provider_error_policy.contains(r#""planType":"cloudProviderErrorClassification""#));
        assert!(provider_error_policy.contains("coarse native/provider rounds"));

        let provider_connection_policy =
            browser_storage_sync_cloud_provider_connection_status_policy_json(
                r#"{"providerKind":"localFolder","connectionState":"connected","credentialState":"available","membershipStatus":"active","cloudStoreReady":true}"#.to_string(),
                0,
                8,
            );
        assert!(provider_connection_policy.contains(r#""classification":"ready""#));
        assert!(provider_connection_policy.contains(r#""shouldAttemptSync":true"#));

        let provider_error_classification =
            browser_storage_sync_cloud_provider_error_classification_json(
                r#"{"providerKind":"googleDrive","errors":[{"operationIndex":1,"operationType":"writeManifest","providerStatusCode":412,"message":"etag mismatch"}]}"#.to_string(),
                0,
                8,
            );
        assert!(
            provider_error_classification
                .contains(r#""classification":"cloudStateRereadRequired""#)
        );
        assert!(provider_error_classification.contains(r#""canonicalErrorKind":"conflict""#));

        let cloud_store_preflight = browser_storage_sync_cloud_store_capability_preflight_json(
            r#"{"providerKind":"memory","rootPath":"Actro/BrowserSync","revisionKind":"monotonicCounter","supportsPrefixList":true,"supportsPagedList":true,"supportsConditionalWrite":true,"supportsConditionalDelete":true,"supportsReadIfChanged":true,"supportsAtomicRename":false,"supportsAppFolder":true,"backgroundSync":"testControlled","conflictResolution":"optimisticRevision","maxObjectBytes":8388608,"maxPageSize":1000}"#.to_string(),
            8,
        );
        assert!(cloud_store_preflight.contains(r#""planType":"cloudStoreCapabilityPreflight""#));
        assert!(cloud_store_preflight.contains(r#""classification":"ready""#));

        let local_folder_capabilities = browser_storage_sync_cloud_local_folder_capabilities_json();
        assert!(local_folder_capabilities.contains(r#""providerKind":"localFolder""#));
        assert!(local_folder_capabilities.contains(r#""revisionKind":"contentHash""#));

        let local_folder_preflight =
            browser_storage_sync_cloud_local_folder_capability_preflight_json(8);
        assert!(local_folder_preflight.contains(r#""planType":"cloudStoreCapabilityPreflight""#));
        assert!(local_folder_preflight.contains(r#""providerKind":"localfolder""#));
        assert!(local_folder_preflight.contains(r#""classification":"ready""#));

        let cloud_manifest = browser_storage_sync_cloud_default_manifest_json(
            "profile".to_string(),
            "device-a".to_string(),
            8,
        );
        let high_watermark_policy = browser_storage_sync_cloud_high_watermark_policy_json();
        assert!(
            high_watermark_policy
                .contains(r#""policyName":"browserStorageSyncCloudHighWatermark""#)
        );
        let high_watermark_plan = browser_storage_sync_cloud_high_watermark_plan_json(
            cloud_manifest.clone(),
            "[]".to_string(),
            "[]".to_string(),
            "[]".to_string(),
            "device-a".to_string(),
            8,
        );
        assert!(high_watermark_plan.contains(r#""planType":"cloudHighWatermarkRetention""#));
        assert!(high_watermark_plan.contains(r#""allCurrent":true"#));
        let cleanup_result_plan = browser_storage_sync_cloud_cleanup_result_plan_json(
            high_watermark_plan.clone(),
            "[]".to_string(),
            8,
        );
        assert!(cleanup_result_plan.contains(r#""planType":"cloudCleanupResult""#));
        assert!(cleanup_result_plan.contains(r#""clientAction":"nothingToCleanup""#));
        let cloud_compatibility_preflight = browser_storage_sync_cloud_compatibility_preflight_json(
            format!(
                r#"[{{"path":"Actro/BrowserSync/manifest.json","document":{cloud_manifest}}}]"#
            ),
            8,
        );
        assert!(
            cloud_compatibility_preflight.contains(r#""planType":"cloudCompatibilityPreflight""#)
        );
        assert!(cloud_compatibility_preflight.contains(r#""classification":"compatible""#));

        let cloud_schema_rollout = browser_storage_sync_cloud_schema_rollout_plan_json(
            cloud_compatibility_preflight.clone(),
            8,
        );
        assert!(cloud_schema_rollout.contains(r#""planType":"cloudSchemaRolloutPlan""#));
        assert!(cloud_schema_rollout.contains(r#""classification":"ready""#));
        assert!(cloud_schema_rollout.contains(r#""compatibilityWindow":"currentOnly""#));

        let cloud_schema_migration_playbook =
            browser_storage_sync_cloud_schema_migration_playbook_json(cloud_schema_rollout, 8);
        assert!(
            cloud_schema_migration_playbook
                .contains(r#""planType":"cloudSchemaMigrationPlaybook""#)
        );
        assert!(
            cloud_schema_migration_playbook.contains(r#""classification":"noMigrationRequired""#)
        );
        assert!(cloud_schema_migration_playbook.contains(r#""shouldAllowNormalSync":true"#));

        let cloud_recovery_snapshot_plan = browser_storage_sync_cloud_recovery_snapshot_plan_json(
            cloud_schema_migration_playbook.clone(),
            cloud_compatibility_preflight.clone(),
            8,
        );
        assert!(cloud_recovery_snapshot_plan.contains(r#""planType":"cloudRecoverySnapshotPlan""#));
        assert!(cloud_recovery_snapshot_plan.contains(r#""shouldSnapshot":false"#));
        let cloud_recovery_snapshot_result =
            browser_storage_sync_cloud_recovery_snapshot_result_plan_json(
                cloud_recovery_snapshot_plan,
                "[]".to_string(),
                8,
            );
        assert!(
            cloud_recovery_snapshot_result.contains(r#""planType":"cloudRecoverySnapshotResult""#)
        );
        assert!(cloud_recovery_snapshot_result.contains(r#""completed":true"#));

        let cloud_recovery_snapshot_round =
            browser_storage_sync_cloud_local_folder_recovery_snapshot_round_json(
                "".to_string(),
                cloud_schema_migration_playbook,
                cloud_compatibility_preflight,
                8,
            );
        assert_eq!(cloud_recovery_snapshot_round, "null");

        let cloud_compaction_round = browser_storage_sync_cloud_local_folder_compaction_round_json(
            "".to_string(),
            "device-a".to_string(),
            8,
        );
        assert_eq!(cloud_compaction_round, "null");

        let cloud_cleanup_round = browser_storage_sync_cloud_local_folder_cleanup_round_json(
            "".to_string(),
            "[]".to_string(),
            "device-a".to_string(),
            8,
        );
        assert_eq!(cloud_cleanup_round, "null");

        let cloud_recovery_retention_round =
            browser_storage_sync_cloud_local_folder_recovery_retention_round_json(
                "".to_string(),
                2,
                8,
            );
        assert_eq!(cloud_recovery_retention_round, "null");

        let provider_diagnostics = browser_storage_sync_cloud_provider_diagnostics_json(
            local_folder_preflight,
            "".to_string(),
            "".to_string(),
            "".to_string(),
            "".to_string(),
            9,
        );
        assert!(provider_diagnostics.contains(r#""planType":"cloudProviderDiagnostics""#));
        assert!(provider_diagnostics.contains(r#""classification":"ready""#));

        let provider_error_classification =
            browser_storage_sync_cloud_provider_error_classification_json(
                r#"{"providerKind":"googleDrive","errors":[{"operationType":"writeManifest","providerStatusCode":412,"message":"etag mismatch"}]}"#.to_string(),
                0,
                10,
            );
        let provider_error_diagnostics = browser_storage_sync_cloud_provider_diagnostics_json(
            "".to_string(),
            "".to_string(),
            provider_error_classification,
            "".to_string(),
            "".to_string(),
            11,
        );
        assert!(provider_error_diagnostics.contains(r#""classification":"cloudConflict""#));
        assert!(
            provider_error_diagnostics
                .contains(r#""providerErrorClassification":"cloudStateRereadRequired""#)
        );
        assert!(provider_error_diagnostics.contains(r#""pendingCloudReread":true"#));

        let cloud_cycle_attempt_plan = browser_storage_sync_cloud_cycle_attempt_plan_json(
            r#"{"scope":"profile","deviceId":"device-a","generatedAtUnixSeconds":7,"completed":false,"shouldRetry":false,"shouldReread":false,"shouldAckLocal":false,"shouldApplyRemote":false,"clientAction":"recoverProviderCredentials","pushRound":{"resultPlan":{"issues":[{"reason":"unauthorized","retryable":false}]}}}"#.to_string(),
            0,
            10_000,
        );
        assert!(cloud_cycle_attempt_plan.contains(r#""planType":"cloudCycleAttemptPlan""#));
        assert!(cloud_cycle_attempt_plan.contains(r#""attemptAction":"reconnectProvider""#));
        assert!(cloud_cycle_attempt_plan.contains(r#""shouldStopUntilRecovery":true"#));

        let cloud_membership_attempt_plan =
            browser_storage_sync_cloud_membership_attempt_plan_json(
                r#"{"scope":"profile","actorDeviceId":"device-a","memberDeviceId":"device-b","membershipAction":"connect","generatedAtUnixSeconds":7,"completed":false,"shouldRetry":false,"shouldReread":false,"clientAction":"recoverProviderCredentials","resultPlan":{"issues":[{"reason":"unauthorized","retryable":false}]}}"#.to_string(),
                0,
                10_000,
            );
        assert!(
            cloud_membership_attempt_plan.contains(r#""planType":"cloudMembershipAttemptPlan""#)
        );
        assert!(cloud_membership_attempt_plan.contains(r#""attemptAction":"reconnectProvider""#));
        assert!(cloud_membership_attempt_plan.contains(r#""shouldStopUntilRecovery":true"#));

        let cloud_membership_loop_plan = browser_storage_sync_cloud_membership_loop_plan_json(
            r#"[{"scope":"profile","actorDeviceId":"device-a","memberDeviceId":"device-b","membershipAction":"connect","generatedAtUnixSeconds":7,"completed":false,"shouldRetry":false,"shouldReread":false,"clientAction":"recoverProviderCredentials","resultPlan":{"issues":[{"reason":"unauthorized","retryable":false}]}}]"#.to_string(),
            0,
            10_000,
            3,
        );
        assert!(cloud_membership_loop_plan.contains(r#""planType":"cloudMembershipLoopPlan""#));
        assert!(cloud_membership_loop_plan.contains(r#""finalAction":"reconnectProvider""#));
        assert!(cloud_membership_loop_plan.contains(r#""shouldStopUntilRecovery":true"#));

        let cloud_cycle_loop_plan = browser_storage_sync_cloud_cycle_loop_plan_json(
            r#"[{"scope":"profile","deviceId":"device-a","generatedAtUnixSeconds":7,"completed":false,"shouldRetry":false,"shouldReread":false,"shouldAckLocal":false,"shouldApplyRemote":false,"clientAction":"recoverProviderCredentials","pushRound":{"resultPlan":{"issues":[{"reason":"unauthorized","retryable":false}]}}}]"#.to_string(),
            0,
            10_000,
            3,
        );
        assert!(cloud_cycle_loop_plan.contains(r#""planType":"cloudCycleLoopPlan""#));
        assert!(cloud_cycle_loop_plan.contains(r#""finalAction":"reconnectProvider""#));
        assert!(cloud_cycle_loop_plan.contains(r#""shouldStopUntilRecovery":true"#));

        let cloud_observability_event = browser_storage_sync_cloud_observability_event_json(
            provider_diagnostics,
            cloud_membership_attempt_plan,
            cloud_membership_loop_plan,
            10_001,
        );
        assert!(cloud_observability_event.contains(r#""planType":"cloudObservabilityEvent""#));
        assert!(
            cloud_observability_event
                .contains(r#""eventName":"browserSyncCloudUserActionRequired""#)
        );
        assert!(cloud_observability_event.contains(r#""redactedExportAllowed":true"#));

        let local_folder_root = std::env::temp_dir().join(format!(
            "actrium-core-ffi-local-folder-cycle-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&local_folder_root);
        let local_folder_cycle_round = browser_storage_sync_cloud_local_folder_cycle_round_json(
            local_folder_root.to_string_lossy().to_string(),
            "null".to_string(),
            "[]".to_string(),
            "device-a".to_string(),
            10_000,
            20,
        );
        let _ = std::fs::remove_dir_all(&local_folder_root);
        assert!(local_folder_cycle_round.contains(r#""planType":"cycleRound""#));

        let request_plan = browser_storage_sync_cycle_request_plan_json(
            "http://127.0.0.1:8787/".to_string(),
            "token-a-123".to_string(),
            browser_storage_sync_push_request_json(empty_browser_storage_sync_batch_json(
                "profile".to_string(),
                "device-a".to_string(),
                6,
            )),
            r#"[{"collection":"historyPages","cursor":"7"}]"#.to_string(),
        );
        assert!(request_plan.contains(r#""method":"POST""#));
        assert!(request_plan.contains("/v1/browser-storage/sync/cycle"));
        assert!(request_plan.contains(r#""Authorization","value":"Bearer token-a-123""#));
        assert!(request_plan.contains(r#""bodyJson""#));

        let transport_result = browser_storage_sync_cycle_transport_result_json(
            200,
            normalized_cycle_response,
            "".to_string(),
        );
        assert!(transport_result.contains(r#""classification":"success""#));
        assert!(transport_result.contains(r#""clientAction":"buildClientCyclePlan""#));

        let retry_result = browser_storage_sync_cycle_transport_result_json(
            503,
            "".to_string(),
            "try later".to_string(),
        );
        assert!(retry_result.contains(r#""classification":"serverUnavailable""#));
        assert!(retry_result.contains(r#""shouldRetry":true"#));

        let status_policy = browser_storage_sync_status_policy_json(
            "serverUnavailable".to_string(),
            true,
            "retryWithBackoff".to_string(),
            2,
        );
        assert!(status_policy.contains(r#""schedulerAction":"scheduleRetry""#));
        assert!(status_policy.contains(r#""retryDelaySeconds":20"#));

        let schedule_state = browser_storage_sync_schedule_state_json(status_policy, 10_000);
        assert!(schedule_state.contains(r#""schedulerAction":"scheduleRetry""#));
        assert!(schedule_state.contains(r#""nextRetryAttempt":3"#));
        assert!(schedule_state.contains(r#""nextRunAtUnixSeconds":10020"#));

        let scheduler_status = browser_storage_sync_scheduler_status_json(
            r#"{"retryAttempt":2,"schedulerAction":"scheduleRetry","nextRunDelaySeconds":20,"nextRunAtUnixSeconds":10020,"pendingRunScheduled":true,"lastResultJson":"{\"classification\":\"serverUnavailable\"}"}"#.to_string(),
        );
        assert!(scheduler_status.contains(r#""status":"retryScheduled""#));
        assert!(scheduler_status.contains(r#""lastClassification":"serverUnavailable""#));
    }

    #[test]
    fn ffi_schema_exports_version_tables_and_tag_seed() {
        assert_eq!(core_schema_version(), 2);
        assert!(core_schema_table_names().contains(&"tab_sessions".to_string()));
        assert!(
            core_schema_statements()
                .iter()
                .any(|statement| statement.contains("CREATE TABLE IF NOT EXISTS history"))
        );
        assert!(
            default_tags()
                .iter()
                .any(|tag| tag.name == "App" && tag.is_system)
        );
        assert_eq!(
            default_system_tags()
                .into_iter()
                .map(|tag| tag.name)
                .collect::<Vec<_>>(),
            vec!["App", "Publisher", "LastUsed", "Frequent"]
        );
        assert_eq!(default_tag_colors().len(), 10);
        assert_eq!(default_tag_colors()[0], "#FF3B30");
    }

    #[test]
    fn ffi_repository_sql_exports_apple_history_and_tag_queries() {
        let history_sql = core_history_repository_sql();
        assert_eq!(
            history_sql.insert_visit,
            "INSERT INTO history (id, title, app_url, app_id, visit_time) VALUES (?, ?, ?, ?, ?)"
        );
        assert_eq!(
            history_sql.select_recent,
            "SELECT * FROM history ORDER BY visit_time DESC LIMIT ?"
        );
        assert_eq!(history_sql.delete_all, "DELETE FROM history");
        assert_eq!(
            core_history_search_title_param("Hello".to_string()).as_deref(),
            Some("%hello%")
        );
        assert_eq!(core_history_search_title_param("".to_string()), None);
        let history_item = core_history_item_from_row(CoreHistoryItemRowInfo {
            id: "history-1".to_string(),
            title: "Example".to_string(),
            app_url: Some("actro://example".to_string()),
            app_id: Some("example".to_string()),
            visit_time_seconds: 1_700_000_000,
        });
        assert_eq!(history_item.id, "history-1");
        assert_eq!(history_item.title, "Example");
        assert_eq!(history_item.app_url.as_deref(), Some("actro://example"));
        assert_eq!(history_item.visit_time_seconds, 1_700_000_000);

        let tag_sql = core_tag_repository_sql();
        assert_eq!(tag_sql.count_all, "SELECT COUNT(*) FROM tags");
        assert_eq!(
            tag_sql.select_all,
            "SELECT * FROM tags ORDER BY is_system DESC, name ASC"
        );
        assert!(tag_sql.select_by_bookmark_id.contains("bookmark_tags"));
        assert_eq!(
            tag_sql.insert_or_ignore_bookmark_tag,
            "INSERT OR IGNORE INTO bookmark_tags (bookmark_id, tag_id) VALUES (?, ?)"
        );
        let tag = core_tag_from_row(CoreTagRowInfo {
            id: "tag-1".to_string(),
            name: "Pinned".to_string(),
            color: Some("#007AFF".to_string()),
            is_system_value: 1,
        });
        assert_eq!(tag.id, "tag-1");
        assert_eq!(tag.name, "Pinned");
        assert_eq!(tag.color.as_deref(), Some("#007AFF"));
        assert!(tag.is_system);
        assert!(
            !core_tag_from_row(CoreTagRowInfo {
                id: "tag-2".to_string(),
                name: "User".to_string(),
                color: None,
                is_system_value: 0,
            })
            .is_system
        );

        let bookmark_sql = core_bookmark_repository_sql();
        assert!(
            bookmark_sql
                .insert_bookmark
                .starts_with("INSERT INTO bookmarks")
        );
        assert_eq!(
            bookmark_sql.select_all,
            "SELECT * FROM bookmarks ORDER BY title ASC"
        );
        assert!(bookmark_sql.select_by_tag.contains("bookmark_tags"));
        assert_eq!(bookmark_sql.vacuum, "VACUUM");
        assert_eq!(
            core_bookmark_search_params("Note".to_string()),
            vec!["%note%".to_string(), "%note%".to_string()]
        );
        assert!(core_bookmark_search_params("".to_string()).is_empty());
        let bookmark = core_bookmark_from_row(CoreBookmarkRowInfo {
            id: "bookmark-1".to_string(),
            title: "Docs".to_string(),
            app_url: Some("actro://docs".to_string()),
            app_id: Some("docs".to_string()),
            created_at_seconds: 1_700_000_001,
            last_used_at_seconds: Some(1_700_000_002),
            publisher: Some("Actrium".to_string()),
            notes: Some("Pinned".to_string()),
            favicon: Some("star.fill".to_string()),
        });
        assert_eq!(bookmark.id, "bookmark-1");
        assert_eq!(bookmark.title, "Docs");
        assert_eq!(bookmark.app_url.as_deref(), Some("actro://docs"));
        assert_eq!(bookmark.last_used_at_seconds, Some(1_700_000_002));

        let app_sql = core_app_repository_sql();
        assert!(
            app_sql
                .insert_or_replace_app
                .starts_with("INSERT OR REPLACE INTO apps")
        );
        assert_eq!(app_sql.select_all, "SELECT * FROM apps ORDER BY name ASC");
        assert!(
            app_sql
                .search_by_name_description_or_bundle_id
                .contains("LOWER(bundle_id)")
        );
        assert_eq!(app_sql.vacuum, "VACUUM");
        assert_eq!(
            core_app_search_params("Local".to_string()),
            vec![
                "%local%".to_string(),
                "%local%".to_string(),
                "%local%".to_string()
            ]
        );
        assert!(core_app_search_params("".to_string()).is_empty());
        let app = core_installed_app_from_row(CoreInstalledAppRowInfo {
            id: Some("app-1".to_string()),
            name: Some("Demo".to_string()),
            bundle_id: Some("com.example.demo".to_string()),
            version: Some("1.0.0".to_string()),
            description: None,
            source_type: Some("localDev".to_string()),
            source_path: Some("/tmp/demo.actr".to_string()),
            icon: Some("app.fill".to_string()),
            installed_at_seconds: Some(1_700_000_003),
            last_used_at_seconds: None,
            is_pinned_value: 1,
        })
        .unwrap();
        assert_eq!(app.id, "app-1");
        assert_eq!(app.source_type, "localDev");
        assert!(app.is_pinned);
        assert!(
            core_installed_app_from_row(CoreInstalledAppRowInfo {
                id: Some("app-2".to_string()),
                name: Some("Broken".to_string()),
                bundle_id: Some("com.example.broken".to_string()),
                version: None,
                description: None,
                source_type: Some("local-dev".to_string()),
                source_path: Some("/tmp/broken.actr".to_string()),
                icon: None,
                installed_at_seconds: Some(1_700_000_004),
                last_used_at_seconds: None,
                is_pinned_value: 0,
            })
            .is_none()
        );
    }

    #[test]
    fn ffi_repository_bind_params_preserve_native_sql_order() {
        let history_params = core_history_insert_visit_params(
            "history-1".to_string(),
            "Example".to_string(),
            Some("actro://example".to_string()),
            None,
            1_700_000_010,
        );
        assert_eq!(history_params.len(), 5);
        assert_eq!(history_params[0].text.as_deref(), Some("history-1"));
        assert_eq!(history_params[2].text.as_deref(), Some("actro://example"));
        assert_eq!(history_params[3].kind, "null");
        assert_eq!(history_params[4].integer, Some(1_700_000_010));

        let tag_params = core_tag_insert_params(
            "tag-1".to_string(),
            "Work".to_string(),
            Some("#34C759".to_string()),
            true,
        );
        assert_eq!(tag_params.len(), 4);
        assert_eq!(tag_params[0].text.as_deref(), Some("tag-1"));
        assert_eq!(tag_params[2].text.as_deref(), Some("#34C759"));
        assert_eq!(tag_params[3].integer, Some(1));
        let tag_update_params =
            core_tag_update_params("Personal".to_string(), None, "tag-2".to_string());
        assert_eq!(tag_update_params.len(), 3);
        assert_eq!(tag_update_params[0].text.as_deref(), Some("Personal"));
        assert_eq!(tag_update_params[1].kind, "null");
        assert_eq!(tag_update_params[2].text.as_deref(), Some("tag-2"));

        let bookmark = CoreBookmarkInfo {
            id: "bookmark-1".to_string(),
            title: "Docs".to_string(),
            app_url: Some("actro://docs".to_string()),
            app_id: Some("docs".to_string()),
            created_at_seconds: 1_700_000_011,
            last_used_at_seconds: Some(1_700_000_012),
            publisher: Some("Actrium".to_string()),
            notes: None,
            favicon: Some("star.fill".to_string()),
        };
        let bookmark_insert_params = core_bookmark_insert_params(bookmark.clone());
        assert_eq!(bookmark_insert_params.len(), 9);
        assert_eq!(
            bookmark_insert_params[0].text.as_deref(),
            Some("bookmark-1")
        );
        assert_eq!(bookmark_insert_params[4].integer, Some(1_700_000_011));
        assert_eq!(bookmark_insert_params[7].kind, "null");
        let bookmark_update_params = core_bookmark_update_params(bookmark);
        assert_eq!(bookmark_update_params.len(), 8);
        assert_eq!(bookmark_update_params[0].text.as_deref(), Some("Docs"));
        assert_eq!(
            bookmark_update_params[7].text.as_deref(),
            Some("bookmark-1")
        );

        let app = CoreInstalledAppInfo {
            id: "app-1".to_string(),
            name: "Demo".to_string(),
            bundle_id: "com.example.demo".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            source_type: "localDev".to_string(),
            source_path: "/tmp/demo.actr".to_string(),
            icon: None,
            installed_at_seconds: 1_700_000_013,
            last_used_at_seconds: Some(1_700_000_014),
            is_pinned: true,
        };
        let app_insert_params = core_installed_app_insert_params(app.clone()).unwrap();
        assert_eq!(app_insert_params.len(), 11);
        assert_eq!(app_insert_params[0].text.as_deref(), Some("app-1"));
        assert_eq!(app_insert_params[5].text.as_deref(), Some("localDev"));
        assert_eq!(app_insert_params[10].integer, Some(1));
        let app_update_params = core_installed_app_update_params(app).unwrap();
        assert_eq!(app_update_params.len(), 8);
        assert_eq!(app_update_params[0].text.as_deref(), Some("Demo"));
        assert_eq!(app_update_params[7].text.as_deref(), Some("app-1"));

        let invalid_app = CoreInstalledAppInfo {
            id: "app-2".to_string(),
            name: "Broken".to_string(),
            bundle_id: "com.example.broken".to_string(),
            version: None,
            description: None,
            source_type: "local-dev".to_string(),
            source_path: "/tmp/broken.actr".to_string(),
            icon: None,
            installed_at_seconds: 1_700_000_015,
            last_used_at_seconds: None,
            is_pinned: false,
        };
        assert!(core_installed_app_insert_params(invalid_app).is_err());
    }

    #[test]
    fn ffi_speech_helpers_match_android_asr_expectations() {
        assert_eq!(asr_model_language_dir("zh-CN".to_string()), "zh");
        assert_eq!(asr_model_language_dir("en-US".to_string()), "en");
        let model_plan = asr_model_plan("zh-CN".to_string());
        assert_eq!(model_plan.language_dir, "zh");
        assert_eq!(model_plan.asset_path, "model/asr/zh");
        assert_eq!(model_plan.storage_relative_path, "model/asr/zh");
        assert_eq!(model_plan.ready_marker, ".model_ready");
        let contract = asr_bridge_contract();
        assert_eq!(contract.module_name, "AsrModule");
        assert_eq!(contract.result_event_name, "onAsrResult");
        assert_eq!(contract.error_event_name, "onAsrError");
        assert_eq!(contract.error_init, "ASR_INIT_ERROR");
        assert_eq!(contract.error_feed, "ASR_FEED_ERROR");
        assert_eq!(contract.error_stop, "ASR_STOP_ERROR");
        assert_eq!(contract.key_text, "text");
        assert_eq!(contract.key_is_final, "isFinal");
        assert_eq!(
            parse_vosk_result_text(r#"{"text":"hello"}"#.to_string()),
            "hello"
        );
        assert_eq!(
            parse_vosk_partial_text(r#"{"partial":"he"}"#.to_string()),
            "he"
        );
        assert_eq!(parse_vosk_result_text("not json".to_string()), "");
        assert_eq!(
            post_process_zh_asr_segments(
                "\u{4eca}\u{5929} \u{5929}\u{6c14} \u{5f88}\u{597d}".to_string(),
                vec![
                    AsrSegmentInfo {
                        word: "\u{4eca}\u{5929}".to_string(),
                        nature: Some("t".to_string()),
                    },
                    AsrSegmentInfo {
                        word: "\u{5929}\u{6c14}".to_string(),
                        nature: Some("n".to_string()),
                    },
                    AsrSegmentInfo {
                        word: "\u{5f88}\u{597d}".to_string(),
                        nature: Some("a".to_string()),
                    },
                ],
            ),
            "\u{4eca}\u{5929}\u{ff0c}\u{5929}\u{6c14}\u{5f88}\u{597d}\u{3002}"
        );
    }

    #[test]
    fn ffi_runtime_app_status_helpers_match_runtime_contract() {
        assert_eq!(runtime_app_status_alive(), "alive");
        assert_eq!(runtime_app_status_dead(), "dead");
        assert_eq!(default_runtime_app_status(), "alive");
        assert_eq!(
            normalize_runtime_app_status("alive".to_string()).as_deref(),
            Some("alive")
        );
        assert_eq!(
            normalize_runtime_app_status("dead".to_string()).as_deref(),
            Some("dead")
        );
        assert_eq!(normalize_runtime_app_status("Dead".to_string()), None);
    }

    #[test]
    fn ffi_active_tab_row_mapping_preserves_persisted_fields() {
        let tab = active_tab_from_row(ActiveTabRowInfo {
            id: "tab-1".to_string(),
            name: "Demo".to_string(),
            icon: "icon.png".to_string(),
            screenshot: "shot.png".to_string(),
            has_unread: true,
            url: "https://example.test".to_string(),
            is_marked: true,
            status: "dead".to_string(),
            sort_order: 7,
        });

        assert_eq!(tab.id, "tab-1");
        assert_eq!(tab.name, "Demo");
        assert!(tab.has_unread);
        assert!(tab.is_marked);
        assert!(!tab.panel_visible);
        assert_eq!(tab.status, "dead");
    }

    #[test]
    fn ffi_active_tab_row_mapping_defaults_unknown_status_to_alive() {
        let row = active_tab_to_row(
            ActiveTabInfo {
                id: "tab-1".to_string(),
                name: "".to_string(),
                icon: "".to_string(),
                screenshot: "".to_string(),
                has_unread: false,
                url: "".to_string(),
                is_marked: false,
                panel_visible: true,
                status: "Dead".to_string(),
            },
            3,
        );

        assert_eq!(row.status, "alive");
        assert_eq!(row.sort_order, 3);
    }

    #[test]
    fn ffi_active_tab_rows_assign_stable_sort_order() {
        let rows = active_tab_rows_from_tabs(vec![
            ActiveTabInfo {
                id: "a".to_string(),
                name: "A".to_string(),
                icon: "".to_string(),
                screenshot: "".to_string(),
                has_unread: false,
                url: "".to_string(),
                is_marked: false,
                panel_visible: false,
                status: "alive".to_string(),
            },
            ActiveTabInfo {
                id: "b".to_string(),
                name: "B".to_string(),
                icon: "".to_string(),
                screenshot: "".to_string(),
                has_unread: true,
                url: "about:blank".to_string(),
                is_marked: true,
                panel_visible: true,
                status: "dead".to_string(),
            },
        ]);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "a");
        assert_eq!(rows[0].sort_order, 0);
        assert_eq!(rows[1].id, "b");
        assert_eq!(rows[1].sort_order, 1);
        assert_eq!(rows[1].status, "dead");
    }

    #[test]
    fn ffi_runtime_tab_update_preserves_android_payload_semantics() {
        let update = runtime_tab_update_from_fields(
            Some("tab-1".to_string()),
            Some("Updated".to_string()),
            None,
            Some("actro://updated".to_string()),
            Some("Dead".to_string()),
        )
        .unwrap();

        assert_eq!(update.tab_id, "tab-1");
        assert_eq!(update.title.as_deref(), Some("Updated"));
        assert_eq!(update.icon, None);
        assert_eq!(update.app_url.as_deref(), Some("actro://updated"));
        assert_eq!(update.status.as_deref(), Some("alive"));

        assert!(runtime_tab_update_from_fields(None, None, None, None, None).is_none());
    }

    #[test]
    fn ffi_runtime_simple_bridge_payloads_preserve_android_contracts() {
        assert_eq!(runtime_bridge_tab_id_key(), "tabId");
        assert_eq!(runtime_bridge_title_key(), "title");
        assert_eq!(runtime_bridge_icon_key(), "icon");
        assert_eq!(runtime_bridge_app_url_key(), "appUrl");
        assert_eq!(runtime_bridge_status_key(), "status");
        assert_eq!(runtime_bridge_unread_key(), "unread");
        assert_eq!(runtime_bridge_marked_key(), "marked");
        assert_eq!(runtime_bridge_is_bookmarked_key(), "isBookmarked");
        assert_eq!(runtime_bridge_visible_key(), "visible");
        assert_eq!(runtime_bridge_container_width_key(), "containerWidth");
        assert_eq!(runtime_bridge_container_height_key(), "containerHeight");
        assert_eq!(
            runtime_tab_entered_fullscreen_event_name(),
            "onTabEnteredFullscreen"
        );
        assert_eq!(
            runtime_tab_exited_fullscreen_event_name(),
            "onTabExitedFullscreen"
        );
        assert_eq!(
            runtime_tab_bookmark_changed_event_name(),
            "onTabBookmarkChanged"
        );
        assert_eq!(
            runtime_extension_panel_visible_change_event_name(),
            "onExtensionPanelVisibleChange"
        );
        assert_eq!(
            runtime_container_size_change_event_name(),
            "onContainerSizeChange"
        );
        assert_eq!(
            runtime_search_input_changed_event_name(),
            "onSearchInputChanged"
        );
        assert_eq!(runtime_auth_context_client_id_key(), "clientId");
        assert_eq!(runtime_auth_context_app_id_key(), "appId");
        assert_eq!(runtime_auth_context_tab_id_key(), "tabId");
        assert_eq!(runtime_auth_context_app_url_key(), "appUrl");
        assert_eq!(auth_environment_dev(), "dev");
        assert_eq!(auth_environment_beta(), "beta");
        assert_eq!(auth_environment_prod(), "prod");
        assert_eq!(
            auth_environment_from_url("https://gate-actrium.actrium-dev.example".to_string()),
            "dev"
        );
        assert_eq!(
            auth_environment_from_app_url("https://gate-actrium.actrium-dev.example".to_string()),
            "prod"
        );
        assert_eq!(
            auth_environment_from_app_url(
                "https://gate-actrium.actrium-dev.example/with/app".to_string()
            ),
            "dev"
        );
        assert_eq!(auth_environment_storage_suffix("beta".to_string()), ".beta");
        assert_eq!(
            auth_account_base_url("dev".to_string()),
            "http://account-actrium.actrium-dev.example"
        );
        assert_eq!(
            auth_resource_uploader_base_url("beta".to_string()),
            "https://resource-uploader.actrium-beta.example"
        );
        assert_eq!(
            auth_access_control_path("app-123".to_string(), "user.cn".to_string()),
            "/api/access_control/app-123/user.cn"
        );
        assert_eq!(
            auth_client_app_path("app-123".to_string()),
            "/api/client/app/app-123"
        );
        assert_eq!(auth_resource_upload_token_path(), "/api/get-token");
        assert_eq!(
            auth_access_control_url_from_app_url(
                "https://gate-actrium.actrium-beta.example:9443/with/app-123".to_string(),
                "app-123".to_string(),
                "user.cn".to_string()
            )
            .as_deref(),
            Some("https://gate-actrium.actrium-beta.example/api/access_control/app-123/user.cn")
        );
        assert_eq!(
            auth_access_control_url_from_app_url(
                "not a url".to_string(),
                "app-123".to_string(),
                "user.cn".to_string()
            ),
            None
        );
        assert!(auth_is_authorized_code(0));
        assert!(!auth_is_authorized_code(10010));
        assert_eq!(auth_access_control_status(true, true, 0), "authorized");
        assert_eq!(
            auth_access_control_status(true, true, 10010),
            "not_authorized"
        );
        assert_eq!(auth_access_control_status(true, false, 0), "not_authorized");
        assert_eq!(auth_access_control_status(false, true, 0), "unknown");
        assert!(auth_api_response_success(0));
        assert!(auth_api_response_success(200));
        assert!(!auth_api_response_success(10010));
        assert!(auth_is_client_app_not_found_code(10010));
        assert!(!auth_is_client_app_not_found_code(0));
        assert!(auth_is_client_app_not_found_message(
            "App Not Found for id".to_string()
        ));
        assert!(auth_is_client_app_not_found_message(
            "\u{5e94}\u{7528}\u{4e0d}\u{5b58}\u{5728}".to_string()
        ));
        assert!(!auth_is_client_app_not_found_message(
            "network error".to_string()
        ));
        assert_eq!(runtime_login_result_event_name(), "onLoginResult");
        assert_eq!(runtime_login_result_type_key(), "type");
        assert_eq!(runtime_login_result_code_key(), "code");
        assert_eq!(runtime_login_result_id_token_key(), "id_token");
        assert_eq!(runtime_login_result_error_key(), "error");
        assert_eq!(runtime_login_result_message_key(), "message");
        assert_eq!(runtime_login_type_wechat(), "wechat");
        assert_eq!(runtime_login_type_apple(), "apple");
        assert_eq!(runtime_login_type_google(), "google");
        assert_eq!(runtime_tab_props_unified_script_key(), "unifiedScript");
        assert_eq!(
            runtime_tab_props_right_panel_default_visible_key(),
            "rightPanelDefaultVisible"
        );
        assert_eq!(runtime_tab_props_id_key(), "id");
        assert_eq!(runtime_tab_props_background_color_key(), "backgroundColor");
        assert_eq!(runtime_tab_props_reload_key_key(), "reloadKey");
        assert_eq!(runtime_tab_props_html_content_key(), "htmlContent");
        assert_eq!(runtime_root_props_tab_key(), "tab");
        assert_eq!(runtime_root_props_tabs_key(), "tabs");
        assert_eq!(runtime_root_props_single_tab_mode_key(), "singleTabMode");
        assert_eq!(runtime_root_props_current_tab_id_key(), "currentTabId");
        assert_eq!(
            runtime_root_props_is_left_panel_visible_key(),
            "isLeftPanelVisible"
        );
        assert_eq!(
            runtime_root_props_is_right_panel_visible_key(),
            "isRightPanelVisible"
        );

        let unread = runtime_tab_unread_from_fields(Some("tab-1".to_string()), Some(true))
            .expect("valid unread payload");
        assert_eq!(unread.tab_id, "tab-1");
        assert!(unread.unread);
        assert!(runtime_tab_unread_from_fields(None, Some(true)).is_none());
        assert!(runtime_tab_unread_from_fields(Some("tab-1".to_string()), None).is_none());

        let bookmark =
            runtime_tab_bookmark_status_from_fields(Some("tab-2".to_string()), Some(false))
                .expect("valid bookmark status payload");
        assert_eq!(bookmark.tab_id, "tab-2");
        assert!(!bookmark.marked);
        assert!(runtime_tab_bookmark_status_from_fields(None, Some(false)).is_none());
        assert!(runtime_tab_bookmark_status_from_fields(Some("tab-2".to_string()), None).is_none());

        let bookmark_event = runtime_tab_bookmark_event("tab-3".to_string(), true);
        assert_eq!(bookmark_event.tab_id, "tab-3");
        assert!(bookmark_event.is_bookmarked);

        let fullscreen = runtime_tab_fullscreen_event("tab-4".to_string());
        assert_eq!(fullscreen.tab_id, "tab-4");

        let panel = runtime_extension_panel_visible_event("tab-5".to_string(), false);
        assert_eq!(panel.tab_id, "tab-5");
        assert!(!panel.visible);

        let size = runtime_container_size_change_event(1000, 800);
        assert_eq!(size.container_width, 1000);
        assert_eq!(size.container_height, 800);
    }

    #[test]
    fn ffi_actr_bridge_contract_preserves_android_and_rn_wire_names() {
        let contract = actr_bridge_contract();

        assert_eq!(contract.module_name, "ActrBridge");
        assert_eq!(contract.stream_event_name, "ActrStreamMessage");
        assert_eq!(contract.status_connected, "connected");
        assert_eq!(contract.stream_type_text, "text");
        assert_eq!(contract.stream_type_voice, "voice");
        assert_eq!(contract.stream_type_attachment_result, "attachmentResult");
        assert_eq!(contract.stream_status_ok, 0);
        assert_eq!(contract.stream_status_error, -1);
        assert_eq!(contract.stream_metadata_eos_key, "eos");
        assert_eq!(contract.stream_metadata_aos_key, "aos");
        assert_eq!(contract.stream_metadata_terminal_value, "true");
        assert_eq!(contract.attachment_result_timeout_ms, 30_000);
        assert_eq!(contract.attachment_chunk_size_bytes, 49_152);
        assert_eq!(contract.attachment_checksum_algorithm_md5, "md5");
        assert_eq!(contract.attachment_status_code_ok, 0);
        assert_eq!(contract.attachment_type_unknown, 0);
        assert_eq!(contract.attachment_type_image, 1);
        assert_eq!(contract.attachment_type_document, 2);
        assert_eq!(contract.attachment_type_audio, 3);
        assert_eq!(contract.attachment_type_video, 4);
        assert_eq!(contract.attachment_type_other, 99);
        assert_eq!(contract.attachment_upload_status_unknown, 0);
        assert_eq!(contract.attachment_upload_status_completed, 1);
        assert_eq!(contract.attachment_upload_status_failed, 2);
        assert_eq!(
            contract.ask_service_prompt_route_key,
            "ask.AskService.Prompt"
        );
        assert_eq!(
            contract.ask_service_attach_route_key,
            "ask.AskService.Attach"
        );
        assert_eq!(
            contract.ask_service_prepare_attachment_upload_route_key,
            "ask.AskService.PrepareAttachmentUpload"
        );
        assert_eq!(
            contract.ask_local_service_send_text_route_key,
            "actrium.AskLocalService.SendText"
        );
        assert_eq!(
            contract.ask_local_service_start_send_voice_route_key,
            "actrium.AskLocalService.StartSendVoice"
        );
        assert_eq!(
            contract.ask_local_service_send_voice_chunk_route_key,
            "actrium.AskLocalService.SendVoiceChunk"
        );
        assert_eq!(contract.ios_ask_service_prompt_timeout_ms, 20_000);
        assert_eq!(
            contract.ios_ask_service_prepare_attachment_upload_timeout_ms,
            20_000
        );
        assert_eq!(contract.android_ask_service_prompt_timeout_ms, 300_000);
        assert_eq!(contract.android_ask_service_attach_timeout_ms, 30_000);
        assert_eq!(contract.android_ask_local_service_rpc_timeout_ms, 30_000);
        assert_eq!(contract.error_invalid_input, "INVALID_INPUT");
        assert_eq!(contract.error_prompt_request, "USR_PROMPT_REQUEST_ERROR");
        assert_eq!(contract.error_not_connected, "NOT_CONNECTED");
        assert_eq!(contract.key_tab_id, "tabId");
        assert_eq!(contract.key_actor_type, "actorType");
        assert_eq!(contract.key_path, "path");
        assert_eq!(contract.key_message_id, "messageId");
        assert_eq!(contract.key_stream_id, "streamId");
        assert_eq!(contract.key_stream_id_upper, "streamID");
        assert_eq!(contract.key_text_response_stream_id, "textResponseStreamId");
        assert_eq!(
            contract.key_voice_response_stream_id,
            "voiceResponseStreamId"
        );
        assert_eq!(contract.key_attachment_id, "attachmentId");
        assert_eq!(contract.key_checksum, "checksum");
        assert_eq!(contract.key_checksum_algorithm, "checksumAlgorithm");
        assert_eq!(contract.key_size_bytes, "sizeBytes");
        assert_eq!(contract.key_result_stream_id, "resultStreamId");
        assert_eq!(contract.key_status_code, "statusCode");
        assert_eq!(contract.key_error_message, "errorMessage");
        assert_eq!(contract.key_timestamp_ms, "timestampMs");
    }

    #[test]
    fn ffi_tts_bridge_contract_preserves_ios_and_rn_wire_names() {
        let contract = tts_bridge_contract();

        assert_eq!(contract.module_name, "TTSModule");
        assert_eq!(contract.event_start, "onTTSStart");
        assert_eq!(contract.event_pause, "onTTSPause");
        assert_eq!(contract.event_resume, "onTTSResume");
        assert_eq!(contract.event_finish, "onTTSFinish");
        assert_eq!(contract.event_error, "onTTSError");
        assert_eq!(contract.key_rate, "rate");
        assert_eq!(contract.key_pitch_multiplier, "pitchMultiplier");
        assert_eq!(contract.key_volume, "volume");
        assert_eq!(contract.key_language, "language");
        assert_eq!(contract.key_voice_identifier, "voiceIdentifier");
        assert_eq!(contract.key_cancelled, "cancelled");
        assert_eq!(contract.key_message, "message");
    }

    #[test]
    fn ffi_email_bridge_contract_preserves_android_and_rn_wire_names() {
        let contract = email_bridge_contract();

        assert_eq!(contract.module_name, "EmailModule");
        assert_eq!(contract.method_compose_email, "composeEmail");
        assert_eq!(contract.uri_scheme_mailto, "mailto:");
        assert_eq!(
            contract.error_invalid_recipients,
            "EMAIL_INVALID_RECIPIENTS"
        );
        assert_eq!(contract.error_open_failed, "EMAIL_OPEN_FAILED");
    }

    #[test]
    fn ffi_user_info_bridge_contract_preserves_native_and_rn_wire_names() {
        let contract = user_info_bridge_contract();

        assert_eq!(contract.module_name, "UserInfoModule");
        assert_eq!(contract.method_get_current_user_info, "getCurrentUserInfo");
        assert_eq!(contract.method_login_with_type, "loginWithType");
        assert_eq!(contract.event_user_info_changed, "onUserInfoChanged");
        assert_eq!(contract.event_login_status_changed, "onLoginStatusChanged");
        assert_eq!(contract.event_user_logout, "onUserLogout");
        assert_eq!(contract.key_id, "id");
        assert_eq!(contract.key_login_api_url, "loginApiUrl");
        assert_eq!(contract.key_user_info, "userInfo");
        assert_eq!(contract.key_user_identity, "userIdentity");
        assert_eq!(contract.key_session_token, "sessionToken");
        assert_eq!(contract.key_created_at, "createdAt");
        assert_eq!(contract.key_updated_at, "updatedAt");
        assert_eq!(contract.key_request_context, "requestContext");
        assert_eq!(contract.key_is_logged_in, "isLoggedIn");
    }

    #[test]
    fn ffi_parse_actr_actor_type_returns_sdk_parts() {
        let actor_type = parse_actr_actor_type(" actrium/app_server@1.0.0 ".to_string()).unwrap();

        assert_eq!(actor_type.publisher, "actrium");
        assert_eq!(actor_type.name, "app_server");
        assert_eq!(actor_type.version, "1.0.0");
    }

    #[test]
    fn ffi_parse_actr_actor_type_rejects_invalid_wire_values() {
        let error = parse_actr_actor_type("actrium+server".to_string()).unwrap_err();

        assert!(matches!(
            error,
            CoreError::InvalidActrActorType(raw) if raw == "actrium+server"
        ));
    }

    #[test]
    fn ffi_runtime_tab_from_active_tab_uses_runtime_defaults() {
        let tab = runtime_tab_from_active_tab(ActiveTabInfo {
            id: "tab-1".to_string(),
            name: "Demo".to_string(),
            icon: "ignored.png".to_string(),
            screenshot: "ignored.png".to_string(),
            has_unread: true,
            url: "https://example.test".to_string(),
            is_marked: true,
            panel_visible: true,
            status: "dead".to_string(),
        });

        assert_eq!(tab.id, "tab-1");
        assert_eq!(tab.title, "Demo");
        assert_eq!(tab.unified_script, "");
        assert!(!tab.right_panel_default_visible);
        assert_eq!(tab.background_color, "#FFFFFF");
        assert_eq!(tab.reload_key, 0);
        assert_eq!(tab.html_content, "");
    }

    #[test]
    fn ffi_runtime_root_props_from_active_tabs_preserves_android_contract() {
        let props = runtime_root_props_from_active_tabs(
            ActiveTabInfo {
                id: "tab-1".to_string(),
                name: "Current".to_string(),
                icon: "".to_string(),
                screenshot: "".to_string(),
                has_unread: false,
                url: "https://example.test/app".to_string(),
                is_marked: false,
                panel_visible: false,
                status: "alive".to_string(),
            },
            vec![
                ActiveTabInfo {
                    id: "tab-1".to_string(),
                    name: "Current".to_string(),
                    icon: "".to_string(),
                    screenshot: "".to_string(),
                    has_unread: false,
                    url: "https://example.test/app".to_string(),
                    is_marked: false,
                    panel_visible: false,
                    status: "alive".to_string(),
                },
                ActiveTabInfo {
                    id: "tab-2".to_string(),
                    name: "Other".to_string(),
                    icon: "".to_string(),
                    screenshot: "".to_string(),
                    has_unread: false,
                    url: "".to_string(),
                    is_marked: false,
                    panel_visible: false,
                    status: "alive".to_string(),
                },
            ],
            800,
            1000,
        );

        assert_eq!(props.current_tab_id, "tab-1");
        assert_eq!(props.tab.title, "Current");
        assert_eq!(props.tabs.len(), 2);
        assert!(props.single_tab_mode);
        assert!(!props.is_left_panel_visible);
        assert!(!props.is_right_panel_visible);
        assert_eq!(props.container_height, 800);
        assert_eq!(props.container_width, 1000);
        assert_eq!(props.app_url.as_deref(), Some("https://example.test/app"));
    }

    #[test]
    fn ffi_runtime_root_props_from_runtime_tabs_can_omit_app_url() {
        let tab = RuntimeTabInfo {
            unified_script: "script".to_string(),
            right_panel_default_visible: true,
            id: "app-1".to_string(),
            background_color: "#111111".to_string(),
            reload_key: 2,
            html_content: "<p>Hello</p>".to_string(),
            title: "App".to_string(),
        };

        let props = runtime_root_props_from_runtime_tabs(
            tab.clone(),
            vec![tab],
            true,
            false,
            640,
            480,
            None,
        );

        assert_eq!(props.current_tab_id, "app-1");
        assert_eq!(props.tab.reload_key, 2);
        assert!(props.is_left_panel_visible);
        assert!(!props.is_right_panel_visible);
        assert_eq!(props.app_url, None);
    }

    #[test]
    fn ffi_runtime_system_root_props_preserves_android_bundle_shape() {
        let props = runtime_system_root_props("settings".to_string()).unwrap();

        assert_eq!(props.tab.system_app_type, "settings");
        assert!(props.single_tab_mode);
    }

    #[test]
    fn ffi_runtime_system_root_props_normalizes_known_aliases() {
        let props = runtime_system_root_props("blank".to_string()).unwrap();

        assert_eq!(props.tab.system_app_type, "startPage");
        assert!(props.single_tab_mode);
    }

    #[test]
    fn ffi_runtime_system_root_props_rejects_unknown_type() {
        let error = runtime_system_root_props("missing".to_string()).unwrap_err();

        assert!(matches!(error, CoreError::UnknownSystemAppType(kind) if kind == "missing"));
    }

    #[test]
    fn ffi_integrity_plan_normalizes_manifest_integrity() {
        let plan = create_actr_integrity_plan(
            r##"
                format = "actr-package/2"
                [actor]
                publisher = "demo"
                name = "app"
                version = "1.0.0"
                [workload]
                profile = "keel.guest/1"
                entry = "dist/main.js"
                [workload.keel]
                runtime = "js"
                guest-api = "keel.guest/1"
                [host]
                profile = "actr.host/1"
                api = "^1"
                [signature]
                algorithm = "ed25519"
                key-id = "publisher-test"
                [[files]]
                path = "dist/main.js"
                role = "workload"
                media-type = "text/javascript"
                size = 1
                sha256 = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            "##
            .to_string(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(plan.algorithm, "sha256");
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].path, "dist/main.js");
        assert_eq!(
            plan.files[0].expected_hash,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn ffi_integrity_plan_rejects_unsafe_paths() {
        let error = create_actr_integrity_plan(
            r##"
                format = "actr-package/2"
                [actor]
                publisher = "demo"
                name = "app"
                version = "1.0.0"
                [workload]
                profile = "keel.guest/1"
                entry = "../secret"
                [workload.keel]
                runtime = "js"
                guest-api = "keel.guest/1"
                [host]
                profile = "actr.host/1"
                api = "^1"
                [signature]
                algorithm = "ed25519"
                key-id = "publisher-test"
                [[files]]
                path = "../secret"
                role = "workload"
                media-type = "text/javascript"
                size = 1
                sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            "##
            .to_string(),
        )
        .unwrap_err();

        assert!(matches!(error, CoreError::InvalidJson(message) if message.contains("../secret")));
    }

    #[test]
    fn ffi_integrity_helpers_match_core_rules() {
        assert_eq!(
            normalize_integrity_algorithm("SHA-256".to_string()).as_deref(),
            Some("sha256")
        );
        assert_eq!(
            normalize_integrity_path("/assets/icon.png".to_string()).as_deref(),
            Some("assets/icon.png")
        );
        assert_eq!(
            normalize_sha256_hex(
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string()
            )
            .as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn ffi_stable_content_fingerprint_matches_sha256_prefix_contract() {
        assert_eq!(
            stable_content_fingerprint("unified-app.js".to_string()),
            "b1a55027d4e0bb5d"
        );
        assert_eq!(stable_content_fingerprint("".to_string()).len(), 16);
    }

    /// A well-formed installation id: 26 lowercase base32 characters
    /// (`a-z`, `2-7`), matching `ActrUri`'s `.actr.alt` domain rules.
    const INSTALLATION_ID: &str = "qscmfu5tbhyeowlknav3zrij4x";

    #[test]
    fn parse_actr_uri_reads_realm_and_actor_links() {
        let realm = parse_actr_uri(
            "actr://Acme.COM/k7m2q9xh".to_string(),
            INSTALLATION_ID.to_string(),
        )
        .unwrap();
        assert_eq!(realm.domain, "acme.com");
        assert_eq!(realm.realm_id.as_deref(), Some("k7m2q9xh"));
        assert_eq!(realm.serial, None);
        assert!(!realm.is_local);
        assert_eq!(
            realm.discovery_url.as_deref(),
            Some("https://acme.com/.well-known/actr")
        );

        let actor = parse_actr_uri(
            "actr://acme.com/2af01.k7m2q9xh/notes?x=1#frag".to_string(),
            INSTALLATION_ID.to_string(),
        )
        .unwrap();
        assert_eq!(actor.realm_id.as_deref(), Some("k7m2q9xh"));
        assert_eq!(actor.serial.as_deref(), Some("2af01"));
    }

    #[test]
    fn parse_actr_uri_resolves_owned_local_realm() {
        let local = parse_actr_uri(
            "actr://qscmfu5tbhyeowlknav3zrij4x.actr.alt/k7m2q9xh".to_string(),
            INSTALLATION_ID.to_string(),
        )
        .unwrap();
        assert!(local.is_local);
        assert_eq!(local.discovery_url, None);
    }

    #[test]
    fn parse_actr_uri_rejects_foreign_local_realm() {
        let error = parse_actr_uri(
            "actr://aaaaaaaaaaaaaaaaaaaaaaaaaa.actr.alt/k7m2q9xh".to_string(),
            INSTALLATION_ID.to_string(),
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::UriLocalRealmForeign));
    }

    #[test]
    fn parse_actr_uri_rejects_authority_only_uri_without_realm_id() {
        // Mirrors `realm_link::open_realm_link`: a Realm link must name an
        // instance; an authority-only URI is not openable.
        let error =
            parse_actr_uri("actr://acme.com".to_string(), INSTALLATION_ID.to_string()).unwrap_err();
        assert!(matches!(error, CoreError::UriLabelInvalid));
    }

    #[test]
    fn parse_actr_uri_rejects_bad_scheme_and_legacy_forms() {
        let scheme = parse_actr_uri(
            "https://acme.com/k7m2q9xh".to_string(),
            INSTALLATION_ID.to_string(),
        )
        .unwrap_err();
        assert!(matches!(scheme, CoreError::UriSchemeInvalid));

        // A legacy form must be rejected, never silently normalized:
        // `actr://<publisher>/<program>@<version>` (uri.rs `is_legacy`).
        let legacy = parse_actr_uri(
            "actr://acme.com/counter@1.0.0".to_string(),
            INSTALLATION_ID.to_string(),
        )
        .unwrap_err();
        assert!(matches!(legacy, CoreError::UriLegacyForm));

        // Ports are not a legacy form; they are a plain domain rejection.
        let port = parse_actr_uri(
            "actr://acme.com:8443/k7m2q9xh".to_string(),
            INSTALLATION_ID.to_string(),
        )
        .unwrap_err();
        assert!(matches!(port, CoreError::UriDomainInvalid));
    }

    #[test]
    fn parse_actr_uri_rejects_invalid_labels() {
        let label = parse_actr_uri(
            "actr://acme.com/Bad_Label".to_string(),
            INSTALLATION_ID.to_string(),
        )
        .unwrap_err();
        assert!(matches!(label, CoreError::UriLabelInvalid));
    }

    #[test]
    fn generated_installation_id_roundtrips_through_validation_and_parsing() {
        let id = generate_installation_id();
        validate_installation_id(id.clone()).unwrap();

        // A generated id opens its own local Realm link end to end.
        let link = format!("actr://{id}.actr.alt/k7m2q9xh");
        let local = parse_actr_uri(link, id).unwrap();
        assert!(local.is_local);
        assert_eq!(local.discovery_url, None);
    }

    #[test]
    fn validate_installation_id_rejects_malformed_ids() {
        assert!(validate_installation_id("short".to_string()).is_err());
        assert!(validate_installation_id("QSCMFU5TBHYEOWLKNAV3ZRIJ4X".to_string()).is_err());
        // `0`, `1`, `8`, `9` are outside the base32 `a-z` + `2-7` alphabet.
        assert!(validate_installation_id("qscmfu5tbhyeowlknav3zrij08".to_string()).is_err());
        assert!(validate_installation_id(INSTALLATION_ID.to_string()).is_ok());
    }

    /// A well-formed descriptor whose `realm_signing_keys` member is vector
    /// `m1` of `contracts/actr/test-vectors/realm-signing-keys-rotation.json`
    /// (single active key `rs-a`, real Ed25519 signature over its JCS form).
    const AUTHORITY_DESCRIPTOR: &str = r#"{
        "version": 1,
        "authority": "actr://acme.example",
        "services": [
            {"rel": "realms", "href": "https://acme.example/realms", "protocol": "actr-realm-profile/1"}
        ],
        "realm_signing_keys": {
            "sequence": 1,
            "keys": [
                {
                    "id": "rs-a",
                    "algorithm": "Ed25519",
                    "public_key": "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w=",
                    "status": "active"
                }
            ],
            "signature": {
                "key_id": "rs-a",
                "algorithm": "Ed25519",
                "value": "srqr147D4MEh4ZaUk2nTH/Gq6PNZt4+UdKQbSEXxRNnAzyzO0EO3L2dRJsICebI3kv64MGsgYICgExy3D67qDw=="
            }
        }
    }"#;

    #[test]
    fn accept_authority_descriptor_extracts_keys_and_realms_href() {
        let info = accept_authority_descriptor(
            AUTHORITY_DESCRIPTOR.to_string(),
            "acme.example".to_string(),
        )
        .unwrap();
        assert_eq!(info.realms_href, "https://acme.example/realms");
        assert_eq!(info.keys_sequence, 1);
        assert_eq!(info.signing_keys.len(), 1);
        assert_eq!(info.signing_keys[0].id, "rs-a");
        assert_eq!(
            info.signing_keys[0].public_key_base64,
            "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w="
        );
        assert_eq!(info.signing_keys[0].status, "active");
    }

    #[test]
    fn accept_authority_descriptor_rejects_authority_mismatch() {
        let error = accept_authority_descriptor(
            AUTHORITY_DESCRIPTOR.to_string(),
            "other.example".to_string(),
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::AuthorityDescriptorInvalid(_)));
    }

    #[test]
    fn accept_authority_descriptor_rejects_tampered_key_signature() {
        // Flip the final base64 character of the member signature.
        let tampered = AUTHORITY_DESCRIPTOR.replacen("qDw==", "qDx==", 1);
        let error = accept_authority_descriptor(tampered, "acme.example".to_string()).unwrap_err();
        assert!(matches!(error, CoreError::AuthorityDescriptorInvalid(_)));
    }

    #[test]
    fn accept_authority_descriptor_requires_realm_signing_keys() {
        let bare = r#"{
            "version": 1,
            "authority": "actr://acme.example",
            "services": [
                {"rel": "realms", "href": "https://acme.example/realms"}
            ]
        }"#;
        let error =
            accept_authority_descriptor(bare.to_string(), "acme.example".to_string()).unwrap_err();
        assert!(matches!(error, CoreError::AuthorityDescriptorInvalid(_)));
    }

    /// `key_sets.initial` of `realm-profile.json`: single active key `rs-a`.
    const PROFILE_SIGNING_KEYS: &str = r#"{
        "sequence": 1,
        "keys": [
            {
                "id": "rs-a",
                "algorithm": "Ed25519",
                "public_key": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
                "status": "active"
            }
        ],
        "signature": {
            "key_id": "rs-a",
            "algorithm": "Ed25519",
            "value": "55ntdTXVOE1J26OoRHLuG5GwdlSFCWSH3wJG2fYSw1kU8Icu80EgJ29sWmEiyzGCrY5Q5dFDJGpZfd7C6l7CDw=="
        }
    }"#;

    /// `profiles.active` of `realm-profile.json`: a signed active Realm
    /// Profile with unknown members (`x-extension`) covered by the signature.
    const REALM_PROFILE_ACTIVE: &str = r#"{
        "schema": "actr.realm/v1",
        "sequence": 31,
        "realm": {
            "name": "k7m2q9xh@acme.com",
            "display_name": "Acme Notes",
            "status": "active",
            "x-extension": {"enabled": true}
        },
        "package": {
            "href": "https://cdn.notes.example/team-notes-1.4.2.actr",
            "sha256": "abababababababababababababababababababababababababababababababab",
            "type": "notes.example/team-notes@1.4.2",
            "x-extension": {"enabled": true}
        },
        "issuer": {
            "url": "https://identity.provider.example/realms/acme.com/k7m2q9xh",
            "keys": [
                {
                    "id": "7",
                    "algorithm": "Ed25519",
                    "public_key": "PUAXw+hDiVqStwqnTRt+vJyYLM8uxJaMwM1V8Sr0Zgw=",
                    "status": "active",
                    "x-extension": {"enabled": true}
                }
            ],
            "x-extension": {"enabled": true}
        },
        "provider": {
            "id": "provider.example",
            "signaling": [
                {
                    "url": "wss://signal.provider.example/ws",
                    "priority": 10,
                    "control_plane_id": "provider.example/v1",
                    "ticket_audience": "signal-a",
                    "x-extension": {"enabled": true}
                }
            ],
            "ice": [
                {"urls": ["stun:stun.provider.example:3478"], "x-extension": {"enabled": true}},
                {"urls": ["turns:turn.provider.example:5349"], "credential_audience": "turn-pool-1"}
            ],
            "x-extension": {"enabled": true}
        },
        "policy": {"membership": "invite-only", "x-extension": {"enabled": true}},
        "issued_at": "2026-07-29T00:00:00Z",
        "x-extension": {
            "": "BMP",
            "😀": "astral",
            "numbers": [1, 1.5],
            "text": "caf\u00e9"
        },
        "signature": {
            "key_id": "rs-a",
            "algorithm": "Ed25519",
            "value": "kBf5aGBkTG3+V+PQzcYCmuyOmYSpjuAeo2dX6E5pmblERN8aNB/VzXU5cdKvhxls9jO4Q4krEpQPjU37MD7DBw=="
        }
    }"#;

    #[test]
    fn accept_realm_profile_extracts_package_target() {
        let info = accept_realm_profile(
            REALM_PROFILE_ACTIVE.to_string(),
            "k7m2q9xh@acme.com".to_string(),
            PROFILE_SIGNING_KEYS.to_string(),
        )
        .unwrap();
        assert_eq!(info.realm_name, "k7m2q9xh@acme.com");
        assert_eq!(info.display_name, "Acme Notes");
        assert_eq!(info.status, "active");
        assert_eq!(info.sequence, 31);
        assert_eq!(
            info.package_href,
            "https://cdn.notes.example/team-notes-1.4.2.actr"
        );
        assert_eq!(
            info.package_sha256,
            "abababababababababababababababababababababababababababababababab"
        );
        assert_eq!(info.package_type, "notes.example/team-notes@1.4.2");
        assert_eq!(
            info.issuer_url,
            "https://identity.provider.example/realms/acme.com/k7m2q9xh"
        );
        assert_eq!(info.provider_id, "provider.example");
    }

    #[test]
    fn accept_realm_profile_rejects_wrong_realm_name() {
        let error = accept_realm_profile(
            REALM_PROFILE_ACTIVE.to_string(),
            "other@acme.com".to_string(),
            PROFILE_SIGNING_KEYS.to_string(),
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::RealmProfileInvalid(_)));
    }

    #[test]
    fn accept_realm_profile_rejects_tampered_document() {
        // A changed display name is covered by the signature.
        let tampered = REALM_PROFILE_ACTIVE.replacen("Acme Notes", "Evil Notes", 1);
        let error = accept_realm_profile(
            tampered,
            "k7m2q9xh@acme.com".to_string(),
            PROFILE_SIGNING_KEYS.to_string(),
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::RealmProfileInvalid(_)));
    }

    #[test]
    fn accept_realm_profile_rejects_untrusted_signing_keys() {
        // The authority keys from the descriptor test do not list `rs-a`, so
        // the profile signature cannot verify against them.
        let error = accept_realm_profile(
            REALM_PROFILE_ACTIVE.to_string(),
            "k7m2q9xh@acme.com".to_string(),
            r#"{
                "sequence": 1,
                "keys": [
                    {
                        "id": "rs-a",
                        "algorithm": "Ed25519",
                        "public_key": "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w=",
                        "status": "active"
                    }
                ],
                "signature": {
                    "key_id": "rs-a",
                    "algorithm": "Ed25519",
                    "value": "srqr147D4MEh4ZaUk2nTH/Gq6PNZt4+UdKQbSEXxRNnAzyzO0EO3L2dRJsICebI3kv64MGsgYICgExy3D67qDw=="
                }
            }"#
            .to_string(),
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::RealmProfileInvalid(_)));
    }

    #[test]
    fn accept_realm_profile_rejects_retired_realm() {
        // `profiles.retired` of `realm-profile.json`: sequence 33, retired,
        // signed by `rs-a` while it is still listed (retired_signer set).
        let retired = r#"{
        "schema": "actr.realm/v1",
        "sequence": 33,
        "realm": {
            "name": "k7m2q9xh@acme.com",
            "display_name": "Acme Notes",
            "status": "retired",
            "x-extension": {"enabled": true}
        },
        "issuer": {
            "url": "https://identity.provider.example/realms/acme.com/k7m2q9xh",
            "keys": [
                {
                    "id": "7",
                    "algorithm": "Ed25519",
                    "public_key": "PUAXw+hDiVqStwqnTRt+vJyYLM8uxJaMwM1V8Sr0Zgw=",
                    "status": "active",
                    "x-extension": {"enabled": true}
                }
            ],
            "x-extension": {"enabled": true}
        },
        "provider": {
            "id": "provider.example",
            "signaling": [
                {
                    "url": "wss://signal.provider.example/ws",
                    "priority": 10,
                    "control_plane_id": "provider.example/v1",
                    "ticket_audience": "signal-a",
                    "x-extension": {"enabled": true}
                }
            ],
            "ice": [
                {"urls": ["stun:stun.provider.example:3478"], "x-extension": {"enabled": true}},
                {"urls": ["turns:turn.provider.example:5349"], "credential_audience": "turn-pool-1"}
            ],
            "x-extension": {"enabled": true}
        },
        "policy": {"membership": "invite-only", "x-extension": {"enabled": true}},
        "issued_at": "2026-07-29T00:00:00Z",
        "x-extension": {
            "": "BMP",
            "😀": "astral",
            "numbers": [1, 1.5],
            "text": "caf\u00e9"
        },
        "signature": {
            "key_id": "rs-a",
            "algorithm": "Ed25519",
            "value": "fLd+XcyASoIgIuwjRDAgOFMSje2SJKlekR/Gq4mb99+u1aTKPpUhohMeacxnPZN3rYXn+mYGgwGoSvULZg42CA=="
        }
    }"#;
        let retired_keys = r#"{
        "sequence": 2,
        "keys": [
            {"id": "rs-a", "algorithm": "Ed25519", "public_key": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=", "status": "retired"},
            {"id": "rs-b", "algorithm": "Ed25519", "public_key": "PUAXw+hDiVqStwqnTRt+vJyYLM8uxJaMwM1V8Sr0Zgw=", "status": "active"}
        ],
        "rotations": [
            {"from": "rs-a", "to": "rs-b", "sequence": 2, "signature": "ZBAuax/1/sewKmIL6ZhnPE7qBIEOEP8GkjclBfm78iEOlbMTllpFXTGGbgdoMDqsC38TLV+Q8vKm4eziXQqbDQ=="}
        ],
        "signature": {
            "key_id": "rs-b",
            "algorithm": "Ed25519",
            "value": "OHQedaw6th8CWM0S/eHWT9cQW8XEQZiwPkVl0Gx5g4WWA1C3iE0AV41encYee2UegjpoC2VmQ0uxmxOi+PQ5Aw=="
        }
    }"#;
        let error = accept_realm_profile(
            retired.to_string(),
            "k7m2q9xh@acme.com".to_string(),
            retired_keys.to_string(),
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::RealmRetired(_)));
    }

    /// A publisher descriptor as received over HTTPS. Step ④ only extracts
    /// the package signing keys, so the profile needs no valid signature
    /// here; verification belongs to step ⑤.
    const PUBLISHER_DESCRIPTOR: &str = r#"{
        "version": 1,
        "authority": "actr://notes.example",
        "publisher": {
            "schema": "actr.publisher/v1",
            "sequence": 1,
            "domain": "notes.example",
            "profile_signing_keys": [
                {
                    "id": "profile-root",
                    "algorithm": "Ed25519",
                    "public_key": "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
                    "status": "active"
                }
            ],
            "keys": [
                {
                    "id": "actr-pub-0123456789abcdef",
                    "algorithm": "Ed25519",
                    "public_key": "PUAXw+hDiVqStwqnTRt+vJyYLM8uxJaMwM1V8Sr0Zgw=",
                    "status": "active"
                }
            ],
            "profile_issued_at": "2026-11-11T00:00:00Z",
            "profile_expires_at": "2027-11-11T00:00:00Z"
        }
    }"#;

    #[test]
    fn accept_publisher_profile_extracts_package_keys() {
        let info = accept_publisher_profile(
            PUBLISHER_DESCRIPTOR.to_string(),
            "notes.example".to_string(),
        )
        .unwrap();
        assert_eq!(info.domain, "notes.example");
        assert_eq!(info.profile_sequence, 1);
        assert_eq!(info.package_signing_keys.len(), 1);
        assert_eq!(info.package_signing_keys[0].id, "actr-pub-0123456789abcdef");
        assert_eq!(info.package_signing_keys[0].status, "active");
    }

    #[test]
    fn accept_publisher_profile_reports_missing_publisher_member() {
        let body = serde_json::json!({
            "version": 1,
            "authority": "actr://notes.example",
            "services": []
        })
        .to_string();
        let error = accept_publisher_profile(body, "notes.example".to_string()).unwrap_err();
        assert!(matches!(error, CoreError::AuthorityDescriptorInvalid(_)));
    }

    #[test]
    fn accept_publisher_profile_reports_missing_signing_keys() {
        let body = serde_json::json!({
            "version": 1,
            "authority": "actr://notes.example",
            "publisher": {"schema": "actr.publisher/v1", "domain": "notes.example"}
        })
        .to_string();
        let error = accept_publisher_profile(body, "notes.example".to_string()).unwrap_err();
        assert!(matches!(error, CoreError::AuthorityDescriptorInvalid(_)));
    }
}
