//! `POST /issuer/membership`: Realm membership requests
//! (`actr/protocols/realm-membership.md` section 4).
//!
//! Each request is an Actr management envelope signed by a Realm client
//! key; `join` is signed by the key it registers. Checks run in the order
//! of section 4.3 so an unauthenticated caller learns nothing about the
//! Realm: envelope, signing key or invite, then role, then target.

use actr_protocol::realm_membership::{
    MAX_LABEL_CHARS, MembershipOp, MembershipRequest, Role, invite_secret_hash,
};
use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use platform::realm::membership::{self, Code, MembershipError, Signer};
use platform::realm::{RealmError, RealmProfileRecord};
use serde_json::{Value, json};

use crate::handlers::{IssuerState, expected_provider_id};
use crate::realm_profile::{ProfileChange, ProfileError};

fn fail<T>(code: Code, message: impl Into<String>) -> Result<T, MembershipError> {
    Err(MembershipError::new(code, message))
}

pub(crate) async fn post_membership(
    State(state): State<IssuerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let no_store = [(header::CACHE_CONTROL, "no-store")];
    match handle(&state, &headers, &body).await {
        Ok(reply) => (no_store, Json(reply)).into_response(),
        Err(error) => {
            if error.code == Code::Unavailable {
                platform::recording::warn!("Membership request failed: {}", error.message);
            }
            let status =
                StatusCode::from_u16(error.code.status()).unwrap_or(StatusCode::BAD_REQUEST);
            (
                status,
                no_store,
                Json(json!({ "error": error.code.as_str(), "message": error.message })),
            )
                .into_response()
        }
    }
}

async fn handle(
    state: &IssuerState,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Value, MembershipError> {
    let invalid = |error: &dyn std::fmt::Display| {
        MembershipError::new(Code::InvalidRequest, error.to_string())
    };
    let request = MembershipRequest::parse(body).map_err(|e| invalid(&e))?;
    let provider = expected_provider_id(state, &request.realm, &request.provider_id, headers).await;
    request
        .check(&provider, Utc::now())
        .map_err(|e| invalid(&e))?;
    let expires_at = chrono::DateTime::parse_from_rfc3339(&request.expires_at)
        .map_err(|e| invalid(&e))?
        .timestamp();

    if request.op == MembershipOp::Join {
        return join(state, &request, expires_at).await;
    }

    let unknown = || MembershipError::new(Code::ClientUnknown, "unknown Realm client key");
    let key = membership::client_key(&request.realm, request.signer_key_id())
        .await?
        .ok_or_else(unknown)?;
    request.verify(&key.public_key).map_err(|_| unknown())?;
    let signer = membership::authenticate(key).await?;
    use_nonce(&request.nonce, expires_at).await?;

    match request.op {
        MembershipOp::Join => unreachable!("handled above"),
        MembershipOp::CreateInvite => {
            let invite_expires_at = request
                .invite_expires_at
                .as_deref()
                .ok_or_else(|| invalid(&"create_invite needs invite_expires_at"))?;
            let invite_expires_at = chrono::DateTime::parse_from_rfc3339(invite_expires_at)
                .map_err(|e| invalid(&e))?
                .timestamp();
            membership::create_invite(
                &signer,
                request.role,
                request.member_id.as_deref(),
                invite_expires_at,
                state.limits,
            )
            .await
            .map(|invite| invite.to_json())
        }
        MembershipOp::RevokeInvite => {
            let invite_id = required(&request.invite_id, "invite_id")?;
            membership::revoke_invite(&signer, invite_id).await
        }
        MembershipOp::ListMembers => membership::list_members(&request.realm, Some(&signer)).await,
        MembershipOp::RevokeClient => {
            let key_id = required(&request.key_id, "key_id")?;
            membership::revoke_client(&signer, key_id).await
        }
        MembershipOp::RemoveMember => {
            let member_id = required(&request.member_id, "member_id")?;
            membership::remove_member(&signer, member_id).await
        }
        MembershipOp::SetRole => {
            let member_id = required(&request.member_id, "member_id")?;
            let role = request
                .role
                .ok_or_else(|| invalid(&"set_role needs role"))?;
            membership::set_role(&signer, member_id, role).await
        }
        MembershipOp::TransferOwner => {
            let member_id = required(&request.member_id, "member_id")?;
            let previous = request.previous_owner_role.unwrap_or(Role::Editor);
            membership::transfer_owner(&signer, member_id, previous).await
        }
        MembershipOp::Leave => membership::leave(&signer).await,
        MembershipOp::UpdateRealm => update_realm(state, &signer, &request).await,
    }
}

fn required<'a>(value: &'a Option<String>, member: &str) -> Result<&'a str, MembershipError> {
    value
        .as_deref()
        .ok_or_else(|| MembershipError::new(Code::InvalidRequest, format!("{member} is missing")))
}

async fn use_nonce(nonce: &str, expires_at: i64) -> Result<(), MembershipError> {
    match platform::realm::management::use_nonce(nonce, expires_at).await {
        Ok(true) => Ok(()),
        Ok(false) => fail(Code::InvalidRequest, "nonce was already used"),
        Err(error) => fail(Code::Unavailable, error.to_string()),
    }
}

/// `join`: the signature proves the new key; the invite is looked up before
/// the nonce is recorded, so anonymous requests cannot fill the nonce table.
async fn join(
    state: &IssuerState,
    request: &MembershipRequest,
    expires_at: i64,
) -> Result<Value, MembershipError> {
    let invalid = |error: &dyn std::fmt::Display| {
        MembershipError::new(Code::InvalidRequest, error.to_string())
    };
    let public_key = request.verify_join().map_err(|e| invalid(&e))?;
    let label = request.label.clone().unwrap_or_default();
    if label.chars().count() > MAX_LABEL_CHARS {
        return fail(
            Code::InvalidRequest,
            format!("label has more than {MAX_LABEL_CHARS} characters"),
        );
    }
    let secret = request
        .invite
        .as_deref()
        .ok_or_else(|| invalid(&"join needs invite"))?;
    let secret_hash = invite_secret_hash(secret).map_err(|e| invalid(&e))?;
    match membership::invite_realm(&secret_hash).await? {
        None => return fail(Code::InviteUnknown, "no such invite"),
        Some(realm) if realm != request.realm => {
            return fail(
                Code::InviteRealmMismatch,
                "the invite belongs to another Realm",
            );
        }
        Some(_) => {}
    }
    use_nonce(&request.nonce, expires_at).await?;
    membership::join(
        &request.realm,
        &secret_hash,
        &public_key,
        &label,
        state.limits,
    )
    .await
}

/// `update_realm`: the owner updates or retires a Realm under this Actr Facility
/// Provider's domain, whose Realm Profile Loran signs. The ownership is
/// checked again where the signed Profile is stored.
async fn update_realm(
    state: &IssuerState,
    signer: &Signer,
    request: &MembershipRequest,
) -> Result<Value, MembershipError> {
    if signer.role != Role::Owner {
        return fail(Code::Forbidden, "only the owner may update the Realm");
    }
    let Some(profiles) = state
        .profiles
        .as_ref()
        .filter(|profiles| profiles.holds_key_for(&signer.realm))
    else {
        return fail(
            Code::Forbidden,
            "the Realm signing key holder updates this Realm",
        );
    };
    match platform::realm::Realm::get(&signer.realm).await? {
        Some(realm) if realm.is_active() => {}
        _ => return fail(Code::RealmUnavailable, "the Realm is not active"),
    }
    let change = match request.status.as_deref() {
        Some("retired") => {
            if request.package.is_some() || request.display_name.is_some() {
                return fail(
                    Code::InvalidRequest,
                    "status retired excludes package and display_name",
                );
            }
            ProfileChange {
                retire: true,
                owner: Some(signer.clone()),
                ..Default::default()
            }
        }
        Some(status) => {
            return fail(
                Code::InvalidRequest,
                format!("status may only be retired, not {status}"),
            );
        }
        None if request.package.is_none() && request.display_name.is_none() => {
            return fail(
                Code::InvalidRequest,
                "update_realm needs package, display_name or status",
            );
        }
        None => ProfileChange {
            package: request.package.clone(),
            display_name: request.display_name.clone(),
            retire: false,
            owner: Some(signer.clone()),
        },
    };
    profiles
        .issue(&signer.realm, change)
        .await
        .map_err(|error| match error {
            ProfileError::Invalid(message) => MembershipError::new(Code::InvalidRequest, message),
            ProfileError::Realm(RealmError::Membership(error)) => error,
            ProfileError::Realm(RealmError::Retired) => {
                MembershipError::new(Code::RealmUnavailable, "the Realm is retired")
            }
            error => MembershipError::new(Code::Unavailable, error.to_string()),
        })?;
    let sequence = RealmProfileRecord::get(&signer.realm)
        .await?
        .map_or(0, |record| record.sequence);
    Ok(json!({ "realm": signer.realm, "sequence": sequence }))
}
