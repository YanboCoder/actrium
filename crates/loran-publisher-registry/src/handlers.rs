use crate::{
    PublisherRegistryError, PublisherRegistryManager,
    manager::{NonceRequest, PublishRequest},
    model::PublisherStatus,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post},
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct PublisherRegistryState {
    pub manager: Arc<PublisherRegistryManager>,
}

/// Public routes — only endpoints that external systems (Hyper runtime, CLI) truly need.
/// These are mounted at `/registry` without authentication.
pub fn create_public_router(state: PublisherRegistryState) -> Router {
    Router::new()
        // Hyper runtime: fetches a Publisher public key for package verification
        .route("/publishers/{name}/verifying-key", get(get_verifying_key))
        // CLI `actr pkg publish`: request a one-time nonce for publish auth
        .route("/packages/nonce", post(request_nonce))
        // CLI `actr pkg publish`: publishes signed packages (authenticated via nonce challenge-response)
        .route("/packages/publish", post(publish_package))
        .with_state(state)
}

/// Admin routes — all management and state-changing operations.
/// These are mounted at `/admin/api/publishers` behind JWT authentication middleware.
pub fn create_admin_router(state: PublisherRegistryState) -> Router {
    Router::new()
        // Publishers register themselves with a signed `register_publisher`
        // request at the Actr Facility Provider's `realm-management` endpoint.
        .route("/{id}/status", get(get_status))
        .route("/resolve/{name}", get(resolve_by_name))
        // Package management
        .route("/packages", get(list_packages))
        .route("/packages/{id}/revoke", post(revoke_package))
        // Admin privileged operations
        .route("/admin/list", get(admin_list))
        .route("/admin/{id}/suspend", post(admin_suspend))
        .route("/admin/{id}/reinstate", post(admin_reinstate))
        .route("/admin/{id}/keys", get(admin_list_keys))
        .route(
            "/admin/keys/{history_id}/revoke",
            post(admin_revoke_historical_key),
        )
        // Emergency revocation of a publisher's live signing key (compromised
        // key); historical-key revoke above only covers rotated-out keys.
        .route(
            "/admin/{id}/keys/active/revoke",
            post(admin_revoke_active_key),
        )
        .route("/admin/{id}", delete(admin_delete))
        .with_state(state)
}

// --- Request / Response types ---

#[derive(Deserialize)]
struct ListPackagesQuery {
    publisher: Option<String>,
}

#[derive(Deserialize)]
struct GetVerifyingKeyQuery {
    pub key_id: Option<String>,
}

// --- Handlers ---

async fn get_status(
    State(s): State<PublisherRegistryState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::model::Publisher>, PublisherRegistryError> {
    Ok(Json(s.manager.get_status(id).await?))
}

async fn resolve_by_name(
    State(s): State<PublisherRegistryState>,
    Path(name): Path<String>,
) -> Result<Json<crate::manager::PublisherPublicInfo>, PublisherRegistryError> {
    Ok(Json(s.manager.resolve_by_name(&name).await?))
}

async fn get_verifying_key(
    State(s): State<PublisherRegistryState>,
    Path(name): Path<String>,
    Query(q): Query<GetVerifyingKeyQuery>,
) -> Result<Json<serde_json::Value>, PublisherRegistryError> {
    let info = if let Some(key_id) = q.key_id {
        s.manager.resolve_key_by_id(&name, &key_id).await?
    } else {
        s.manager.resolve_by_name(&name).await?
    };
    Ok(Json(serde_json::json!({
        "public_key": info.public_key,
        "key_id": info.key_id,
    })))
}

/// POST /registry/packages/nonce — request one publish-auth nonce.
async fn request_nonce(
    State(s): State<PublisherRegistryState>,
    Json(req): Json<NonceRequest>,
) -> Result<Json<serde_json::Value>, PublisherRegistryError> {
    use base64::Engine as _;
    let nonce_bytes = s.manager.request_nonce(&req.publisher).await?;
    let nonce_b64 = base64::engine::general_purpose::STANDARD.encode(&nonce_bytes);
    Ok(Json(serde_json::json!({ "nonce": nonce_b64 })))
}

async fn publish_package(
    State(s): State<PublisherRegistryState>,
    Json(req): Json<PublishRequest>,
) -> Result<Json<crate::model::ActrPackage>, PublisherRegistryError> {
    Ok(Json(s.manager.publish_package(req).await?))
}

async fn list_packages(
    State(s): State<PublisherRegistryState>,
    Query(q): Query<ListPackagesQuery>,
) -> Result<Json<Vec<crate::model::ActrPackage>>, PublisherRegistryError> {
    Ok(Json(s.manager.list_packages(q.publisher.as_deref()).await?))
}

async fn revoke_package(
    State(s): State<PublisherRegistryState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, PublisherRegistryError> {
    s.manager.revoke_package(id).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn admin_list(
    State(s): State<PublisherRegistryState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<crate::model::Publisher>>, PublisherRegistryError> {
    let status = q
        .get("status")
        .and_then(|v| v.parse::<PublisherStatus>().ok());
    Ok(Json(s.manager.admin_list(status).await?))
}

async fn admin_suspend(
    State(s): State<PublisherRegistryState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, PublisherRegistryError> {
    s.manager.admin_suspend(id).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn admin_reinstate(
    State(s): State<PublisherRegistryState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, PublisherRegistryError> {
    s.manager.admin_reinstate(id).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn admin_list_keys(
    State(s): State<PublisherRegistryState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<crate::model::key_history::PublisherKeyHistory>>, PublisherRegistryError> {
    let history = s.manager.admin_list_keys(id).await?;
    Ok(Json(history))
}

async fn admin_revoke_historical_key(
    State(s): State<PublisherRegistryState>,
    Path(history_id): Path<i64>,
) -> Result<Json<serde_json::Value>, PublisherRegistryError> {
    s.manager.admin_revoke_historical_key(history_id).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn admin_revoke_active_key(
    State(s): State<PublisherRegistryState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, PublisherRegistryError> {
    s.manager.admin_revoke_active_key(id).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn admin_delete(
    State(s): State<PublisherRegistryState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, PublisherRegistryError> {
    s.manager.admin_delete(id).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}
