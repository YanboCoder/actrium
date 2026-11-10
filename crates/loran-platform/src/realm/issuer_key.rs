//! Realm issuer keys this Actr Facility Provider holds in the Signer
//! (`actr/protocols/realm-profile.md` section 6.1).
//!
//! A Realm has one `current` key that signs Actor credentials. Rotation adds
//! a `next` key, listed as `active` in the Realm Profile while the current
//! key keeps signing; once a Profile listing it has been accepted for the
//! pre-publish period, `next` becomes `current` and the old key `retired`,
//! still verifying until `remove_after`.

use actr_protocol::KeyStatus;
use actr_protocol::realm_profile::RealmProfile;

use super::error::RealmError;
use crate::storage::db::get_database;

/// Continuous publication required before an issuer key may sign credentials.
pub const PREPUBLISH_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssuerKeyState {
    Next,
    Current,
    Retired,
}

impl IssuerKeyState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Current => "current",
            Self::Retired => "retired",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RealmIssuerKey {
    pub realm: String,
    /// Signer key id: the credential `key_id`.
    pub key_id: u32,
    pub signer_name: String,
    /// Base64 Ed25519 public key.
    pub public_key: String,
    pub state: IssuerKeyState,
    pub created_at: i64,
    /// Since when the accepted Realm Profiles have listed this key.
    pub published_at: Option<i64>,
    pub current_since: Option<i64>,
    /// Retired: when the Realm Profile may stop listing it.
    pub remove_after: Option<i64>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for RealmIssuerKey {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        let state = match row.try_get::<String, _>("state")?.as_str() {
            "next" => IssuerKeyState::Next,
            "current" => IssuerKeyState::Current,
            _ => IssuerKeyState::Retired,
        };
        Ok(Self {
            realm: row.try_get("realm")?,
            key_id: row.try_get::<i64, _>("key_id")? as u32,
            signer_name: row.try_get("signer_name")?,
            public_key: row.try_get("public_key")?,
            state,
            created_at: row.try_get("created_at")?,
            published_at: row.try_get("published_at")?,
            current_since: row.try_get("current_since")?,
            remove_after: row.try_get("remove_after")?,
        })
    }
}

impl RealmIssuerKey {
    /// The Realm Profile `issuer.keys` status of this key.
    pub fn profile_status(&self) -> KeyStatus {
        match self.state {
            IssuerKeyState::Retired => KeyStatus::Retired,
            _ => KeyStatus::Active,
        }
    }

    /// The keys of `realm`, oldest first.
    pub async fn list(realm: &str) -> Result<Vec<Self>, RealmError> {
        Ok(sqlx::query_as::<_, Self>(
            "SELECT * FROM realm_issuer_key WHERE realm = ? ORDER BY created_at, key_id",
        )
        .bind(realm)
        .fetch_all(get_database().get_pool())
        .await?)
    }

    pub async fn current(realm: &str) -> Result<Option<Self>, RealmError> {
        Ok(sqlx::query_as::<_, Self>(
            "SELECT * FROM realm_issuer_key WHERE realm = ? AND state = 'current'",
        )
        .bind(realm)
        .fetch_optional(get_database().get_pool())
        .await?)
    }

    /// Insert a new key. Fails with [`RealmError::Conflict`] when `realm`
    /// already has a key in this state (other than `retired`), and with
    /// [`RealmError::Retired`] once the Realm is retired or gone.
    pub async fn insert(&self) -> Result<(), RealmError> {
        let inserted = sqlx::query(
            "INSERT INTO realm_issuer_key
                (realm, key_id, signer_name, public_key, state, created_at, current_since)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7
             WHERE EXISTS (SELECT 1 FROM realm WHERE name = ?1 AND status != 'retired')",
        )
        .bind(&self.realm)
        .bind(self.key_id as i64)
        .bind(&self.signer_name)
        .bind(&self.public_key)
        .bind(self.state.as_str())
        .bind(self.created_at)
        .bind(self.current_since)
        .execute(get_database().get_pool())
        .await
        .map_err(|error| match error {
            sqlx::Error::Database(e) if e.is_unique_violation() => RealmError::Conflict(format!(
                "Realm {} already has a {} issuer key",
                self.realm,
                self.state.as_str()
            )),
            error => error.into(),
        })?
        .rows_affected();
        if inserted == 0 {
            return Err(RealmError::Retired);
        }
        Ok(())
    }

    /// Record that an accepted Realm Profile lists `profile`'s active issuer
    /// keys, in the transaction that stores it. A `next` key it does not list
    /// starts its pre-publish period over once listed again.
    pub(crate) async fn mark_published(
        tx: &mut sqlx::SqliteConnection,
        profile: &RealmProfile,
        now: i64,
    ) -> Result<(), RealmError> {
        let next: Option<(i64, String)> = sqlx::query_as(
            "SELECT key_id, public_key FROM realm_issuer_key WHERE realm = ? AND state = 'next'",
        )
        .bind(&profile.realm.name)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((key_id, public_key)) = next
            && !profile
                .issuer_key(key_id as u32)
                .is_some_and(|key| key.status == KeyStatus::Active && key.public_key == public_key)
        {
            sqlx::query(
                "UPDATE realm_issuer_key SET published_at = NULL
                 WHERE realm = ? AND state = 'next'",
            )
            .bind(&profile.realm.name)
            .execute(&mut *tx)
            .await?;
        }
        for key in &profile.issuer.keys {
            let Some(key_id) = actr_protocol::realm_profile::parse_issuer_key_id(&key.id) else {
                continue;
            };
            if key.status != KeyStatus::Active {
                continue;
            }
            sqlx::query(
                "UPDATE realm_issuer_key SET published_at = ?
                 WHERE realm = ? AND key_id = ? AND public_key = ? AND published_at IS NULL",
            )
            .bind(now)
            .bind(&profile.realm.name)
            .bind(key_id as i64)
            .bind(&key.public_key)
            .execute(&mut *tx)
            .await?;
        }
        Ok(())
    }

    /// Activate this observed next key only if the accepted Profile and its
    /// continuous publication interval still authorize it at commit time.
    pub async fn switch(&self, now: i64, remove_after: i64) -> Result<(), RealmError> {
        let realm = self.realm.as_str();
        let conflict =
            || RealmError::Conflict(format!("Realm {realm} issuer key publication changed"));
        let Some(published_at) = self.published_at else {
            return Err(conflict());
        };
        if self.state != IssuerKeyState::Next || published_at > now.saturating_sub(PREPUBLISH_SECS)
        {
            return Err(conflict());
        }
        // Serialize the final authorization check with Profile acceptance and
        // withdrawal, including the timestamp reset performed by mark_published.
        let mut tx = get_database()
            .get_pool()
            .begin_with("BEGIN IMMEDIATE")
            .await?;
        let document: Option<String> = sqlx::query_scalar(
            "SELECT p.document FROM realm_profile p JOIN realm r ON r.name = p.realm
             WHERE p.realm = ? AND r.status = 'active'",
        )
        .bind(realm)
        .fetch_optional(&mut *tx)
        .await?;
        let profile: RealmProfile = serde_json::from_str(&document.ok_or_else(conflict)?)
            .map_err(|error| RealmError::ParseError(error.to_string()))?;
        if profile.realm.name != realm
            || profile.realm.status != actr_protocol::realm_profile::RealmStatus::Active
            || !profile.issuer_key(self.key_id).is_some_and(|key| {
                key.status == KeyStatus::Active && key.public_key == self.public_key
            })
        {
            return Err(conflict());
        }
        sqlx::query(
            "UPDATE realm_issuer_key SET state = 'retired', remove_after = ?
             WHERE realm = ? AND state = 'current'",
        )
        .bind(remove_after)
        .bind(realm)
        .execute(&mut *tx)
        .await?;
        let switched = sqlx::query(
            "UPDATE realm_issuer_key SET state = 'current', current_since = ?
             WHERE realm = ? AND state = 'next' AND key_id = ? AND public_key = ?
               AND published_at = ? AND published_at <= ?",
        )
        .bind(now)
        .bind(realm)
        .bind(self.key_id as i64)
        .bind(&self.public_key)
        .bind(published_at)
        .bind(now.saturating_sub(PREPUBLISH_SECS))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if switched != 1 {
            return Err(conflict());
        }
        tx.commit().await?;
        Ok(())
    }

    /// Forget a retired key that no Realm Profile needs to list any more.
    pub async fn remove(realm: &str, key_id: u32) -> Result<(), RealmError> {
        sqlx::query(
            "DELETE FROM realm_issuer_key WHERE realm = ? AND key_id = ? AND state = 'retired'",
        )
        .bind(realm)
        .bind(key_id as i64)
        .execute(get_database().get_pool())
        .await?;
        Ok(())
    }
}
