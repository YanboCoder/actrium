use crate::PublisherRegistryError;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PublisherStatus {
    #[default]
    Pending,
    Active,
    Suspended,
    Revoked,
}

impl std::fmt::Display for PublisherStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PublisherStatus::Pending => write!(f, "pending"),
            PublisherStatus::Active => write!(f, "active"),
            PublisherStatus::Suspended => write!(f, "suspended"),
            PublisherStatus::Revoked => write!(f, "revoked"),
        }
    }
}

impl std::str::FromStr for PublisherStatus {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "pending" => Ok(PublisherStatus::Pending),
            "active" => Ok(PublisherStatus::Active),
            "suspended" => Ok(PublisherStatus::Suspended),
            "revoked" => Ok(PublisherStatus::Revoked),
            _ => Err(()),
        }
    }
}

/// Publisher record.
/// `name` is the canonical HTTPS publisher domain and is the identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Publisher {
    pub id: i64,
    /// Canonical DNS domain, e.g. `apps.example.com`.
    pub name: String,
    pub public_key: String,
    pub key_id: String,
    /// Accepted `actr.publisher/v1` Publisher Profile, `{}` before one.
    pub profile_json: String,
    pub contact: Option<String>,
    pub status: PublisherStatus,
    pub created_at: i64,
    pub updated_at: Option<i64>,
    pub verified_at: Option<i64>,
    pub suspended_at: Option<i64>,
    pub revoked_at: Option<i64>,
    /// Signing key validity limit (unix timestamp), accepted from its Profile.
    pub key_expires_at: Option<i64>,
    /// A second `active` package signing key while the publisher rotates
    /// (`actor-package.md` section 6.8); it also verifies packages.
    pub next_public_key: Option<String>,
    pub next_key_id: Option<String>,
    pub next_key_expires_at: Option<i64>,
    /// Pins of the Publisher Profile signing keys (`KeyPins` JSON).
    pub profile_pins: Option<String>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Publisher {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        let status_str: String = row.try_get("status")?;
        let status = status_str.parse().unwrap_or_default();
        Ok(Publisher {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            public_key: row.try_get("public_key")?,
            key_id: row.try_get("key_id").unwrap_or_default(),
            profile_json: row.try_get("profile_json")?,
            contact: row.try_get("contact")?,
            status,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            verified_at: row.try_get("verified_at")?,
            suspended_at: row.try_get("suspended_at")?,
            revoked_at: row.try_get("revoked_at")?,
            key_expires_at: row.try_get("key_expires_at").unwrap_or(None),
            next_public_key: row.try_get("next_public_key")?,
            next_key_id: row.try_get("next_key_id")?,
            next_key_expires_at: row.try_get("next_key_expires_at")?,
            profile_pins: row.try_get("profile_pins")?,
        })
    }
}

impl Publisher {
    /// A pending record; production records come from
    /// `PublisherRegistryManager::register_publisher`.
    #[cfg(test)]
    pub async fn create(
        pool: &SqlitePool,
        name: &str,
        contact: Option<&str>,
    ) -> Result<Self, PublisherRegistryError> {
        let now = Utc::now().timestamp();
        let id = sqlx::query(
            "INSERT INTO publisher (name, contact, status, created_at) VALUES (?, ?, 'pending', ?)",
        )
        .bind(name)
        .bind(contact)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                PublisherRegistryError::AlreadyExists(format!("'{}' already registered", name))
            } else {
                PublisherRegistryError::Database(e)
            }
        })?
        .last_insert_rowid();

        Self::get(pool, id)
            .await?
            .ok_or(PublisherRegistryError::NotFound)
    }

    pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<Self>, PublisherRegistryError> {
        Ok(
            sqlx::query_as::<_, Publisher>("SELECT * FROM publisher WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?,
        )
    }

    pub async fn get_by_name(
        pool: &SqlitePool,
        name: &str,
    ) -> Result<Option<Self>, PublisherRegistryError> {
        Ok(
            sqlx::query_as::<_, Publisher>("SELECT * FROM publisher WHERE name = ?")
                .bind(name)
                .fetch_optional(pool)
                .await?,
        )
    }

    pub async fn list(
        pool: &SqlitePool,
        status: Option<PublisherStatus>,
    ) -> Result<Vec<Self>, PublisherRegistryError> {
        if let Some(s) = status {
            Ok(sqlx::query_as::<_, Publisher>(
                "SELECT * FROM publisher WHERE status = ? ORDER BY created_at DESC",
            )
            .bind(s.to_string())
            .fetch_all(pool)
            .await?)
        } else {
            Ok(
                sqlx::query_as::<_, Publisher>("SELECT * FROM publisher ORDER BY created_at DESC")
                    .fetch_all(pool)
                    .await?,
            )
        }
    }

    #[cfg(test)]
    pub async fn activate(
        &mut self,
        pool: &SqlitePool,
        public_key: String,
    ) -> Result<(), PublisherRegistryError> {
        if self.status != PublisherStatus::Pending {
            return Err(PublisherRegistryError::InvalidStatus(format!(
                "cannot activate from status: {}",
                self.status
            )));
        }
        let now = Utc::now().timestamp();
        let new_key_id = crate::crypto::compute_key_id_from_b64(&public_key)?;
        let key_expires_at = now + 365 * 24 * 3600; // 1 year
        sqlx::query(
            "UPDATE publisher SET status = 'active', public_key = ?, key_id = ?, verified_at = ?, updated_at = ?, key_expires_at = ? WHERE id = ?",
        )
        .bind(&public_key)
        .bind(&new_key_id)
        .bind(now)
        .bind(now)
        .bind(key_expires_at)
        .bind(self.id)
        .execute(pool)
        .await?;
        self.status = PublisherStatus::Active;
        self.public_key = public_key;
        self.key_id = new_key_id;
        self.verified_at = Some(now);
        self.updated_at = Some(now);
        self.key_expires_at = Some(key_expires_at);
        Ok(())
    }

    /// Emergency-revoke the CURRENTLY ACTIVE signing keys (private key
    /// compromised): the current key and, while rotating, the second one.
    ///
    /// `PublisherKeyHistory::revoke` only touches keys that were already rotated away;
    /// this closes the gap where the live key itself is compromised and must be
    /// killed immediately. The operation is atomic:
    ///   1. the live key is archived into `publisher_key_history` with status `revoked`
    ///      (not `retired`), so both exact-key resolution (`is_key_id_revoked`,
    ///      `get_by_key_id`) and the rollback blacklist (`key_id_ever_used`)
    ///      reject it forever, and
    ///   2. the key material and expiry are cleared from the publisher record.
    ///
    /// Coherent post-state: the publisher is deliberately LEFT in `active` status
    /// but WITHOUT a usable key. With `public_key`/`key_id` blanked and
    /// `key_expires_at` set to NULL, every current-key path fails closed -
    /// `resolve_by_name` returns `CertificateExpired`, and
    /// `resolve_key_by_id` for the old key_id returns
    /// `KeyRevoked`. Recovery is only possible by registering a later
    /// Publisher Profile, which requires `active` status and, thanks to the
    /// blacklist, cannot reinstate the revoked key - the publisher must rotate
    /// to a brand-new key.
    ///
    /// Doing both writes in one transaction is required for safety: a half-applied
    /// state where the key is archived as revoked but still present on the `publisher`
    /// row would let `resolve_by_name` (which does not consult the revoked set)
    /// keep serving the compromised key.
    pub async fn revoke_active_key(
        &mut self,
        pool: &SqlitePool,
    ) -> Result<(), PublisherRegistryError> {
        if self.status != PublisherStatus::Active {
            return Err(PublisherRegistryError::InvalidStatus(format!(
                "cannot revoke active key from status: {}",
                self.status
            )));
        }
        if self.public_key.is_empty() || self.key_id.is_empty() {
            return Err(PublisherRegistryError::InvalidStatus(
                "publisher has no active key to revoke".to_string(),
            ));
        }

        let now = Utc::now().timestamp();
        let created_at = self.verified_at.unwrap_or(self.created_at);
        let mut tx = pool.begin().await?;

        // Archive the live key directly as `revoked`: a compromised key must be
        // rejected by all future verification, not merely retired. The UNIQUE
        // (publisher_id, key_id) index cannot conflict here because the current key has
        // never been archived while it was in use.
        let live = std::iter::once((&self.key_id, &self.public_key))
            .chain(self.next_key_id.as_ref().zip(self.next_public_key.as_ref()));
        for (key_id, public_key) in live {
            sqlx::query(
                "INSERT INTO publisher_key_history
                 (publisher_id, key_id, public_key, status, created_at, retired_at)
                 VALUES (?, ?, ?, 'revoked', ?, ?)",
            )
            .bind(self.id)
            .bind(key_id)
            .bind(public_key)
            .bind(created_at)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        // Blank the live key material and expiry so no resolution path can serve
        // it until the Publisher registers a fresh key.
        sqlx::query(
            "UPDATE publisher SET public_key = '', key_id = '', key_expires_at = NULL,
                 next_public_key = NULL, next_key_id = NULL, next_key_expires_at = NULL,
                 updated_at = ? WHERE id = ?",
        )
        .bind(now)
        .bind(self.id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        self.public_key = String::new();
        self.key_id = String::new();
        self.key_expires_at = None;
        self.next_public_key = None;
        self.next_key_id = None;
        self.next_key_expires_at = None;
        self.updated_at = Some(now);
        Ok(())
    }

    pub async fn suspend(&mut self, pool: &SqlitePool) -> Result<(), PublisherRegistryError> {
        if self.status != PublisherStatus::Active {
            return Err(PublisherRegistryError::InvalidStatus(format!(
                "cannot suspend from status: {}",
                self.status
            )));
        }
        let now = Utc::now().timestamp();
        sqlx::query(
            "UPDATE publisher SET status = 'suspended', suspended_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(now)
        .bind(now)
        .bind(self.id)
        .execute(pool)
        .await?;
        self.status = PublisherStatus::Suspended;
        self.suspended_at = Some(now);
        Ok(())
    }

    pub async fn reinstate(&mut self, pool: &SqlitePool) -> Result<(), PublisherRegistryError> {
        if self.status != PublisherStatus::Suspended {
            return Err(PublisherRegistryError::InvalidStatus(format!(
                "cannot reinstate from status: {}",
                self.status
            )));
        }
        let now = Utc::now().timestamp();
        sqlx::query("UPDATE publisher SET status = 'active', updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(self.id)
            .execute(pool)
            .await?;
        self.status = PublisherStatus::Active;
        self.updated_at = Some(now);
        Ok(())
    }

    pub async fn revoke(&mut self, pool: &SqlitePool) -> Result<(), PublisherRegistryError> {
        let now = Utc::now().timestamp();
        sqlx::query(
            "UPDATE publisher SET status = 'revoked', revoked_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(now)
        .bind(now)
        .bind(self.id)
        .execute(pool)
        .await?;
        self.status = PublisherStatus::Revoked;
        self.revoked_at = Some(now);
        Ok(())
    }

    /// Delete the publisher and every row that references it, all or
    /// nothing.
    pub async fn delete(pool: &SqlitePool, id: i64) -> Result<(), PublisherRegistryError> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("DELETE FROM publisher_key_history WHERE publisher_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM publisher WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
