//! Database connection and operation management
//!
//! Provides a sqlx-based database connection pool and basic operations

use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

const LORAN_SCHEMA_VERSION: i64 = 9;

/// Database manager
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Create a new database instance
    ///
    /// # Arguments
    /// * `path` - Directory path for database file storage; must already exist
    ///   The main database file will be stored as `{path}/loran.db`
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db_file = path.as_ref().join("loran.db");

        // Create connection options and enable WAL mode
        let options = SqliteConnectOptions::from_str(&format!("sqlite:{}", db_file.display()))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));

        // Create connection pool
        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .connect_with(options)
            .await?;

        let db = Self { pool };

        // Schema changes are explicit and fail closed. Databases from another
        // schema version must be rebuilt.
        db.verify_schema_version().await?;
        db.initialize_schema().await?;

        Ok(db)
    }

    async fn verify_schema_version(&self) -> Result<()> {
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&self.pool)
            .await?;
        if version == LORAN_SCHEMA_VERSION {
            return Ok(());
        }

        if version == 0 {
            let application_table_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .fetch_one(&self.pool)
            .await?;
            if application_table_count == 0 {
                return Ok(());
            }
        }

        anyhow::bail!(
            "unsupported Loran database schema version {version}; expected \
             {LORAN_SCHEMA_VERSION}. Stop Loran and rebuild the database"
        );
    }

    /// Initialize the database schema.
    ///
    /// All table/index DDL and the `PRAGMA user_version` bump run inside a single
    /// transaction so a mid-initialization crash can never leave a half-built
    /// database with tables present but `user_version = 0` (which the startup gate
    /// rejects and the migration tool cannot repair). SQLite supports
    /// transactional DDL and `PRAGMA user_version` participates in the enclosing
    /// transaction, so the whole schema either lands atomically or rolls back.
    /// Every statement stays idempotent (`IF NOT EXISTS`) so re-running is safe.
    async fn initialize_schema(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // Create the Realm table, keyed by Realm name `<realm_id>@<domain>`.
        // Callers validate the name before writing; the CHECK is a backstop.
        //
        // A retired row is the Actr Facility Provider tombstone: it is never deleted,
        // so the primary key keeps the name from being reused. `enabled` is the
        // operator's local suspend switch and is independent of `status`.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS realm (
                name TEXT PRIMARY KEY CHECK(name LIKE '%_@_%'),
                status TEXT NOT NULL DEFAULT 'active'
                    CHECK(status IN ('pending', 'active', 'retired')),
                enabled INTEGER NOT NULL DEFAULT 1,
                display_name TEXT,
                expires_at INTEGER,
                pending_expires_at INTEGER,
                requester TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER
            )",
        )
        .execute(&mut *tx)
        .await?;

        // A Realm name is immutable, and a retired Realm never comes back.
        sqlx::query(
            "CREATE TRIGGER IF NOT EXISTS realm_name_immutable
             BEFORE UPDATE OF name ON realm
             WHEN NEW.name != OLD.name
             BEGIN SELECT RAISE(ABORT, 'realm name is immutable'); END",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE TRIGGER IF NOT EXISTS realm_retired_is_final
             BEFORE UPDATE OF status ON realm
             WHEN OLD.status = 'retired' AND NEW.status != 'retired'
             BEGIN SELECT RAISE(ABORT, 'retired realm cannot change status'); END",
        )
        .execute(&mut *tx)
        .await?;

        // Accepted `realm_signing_keys` of a Realm domain: the member as
        // received and the key pins (every key ever accepted, with its last
        // status, and the accepted sequence).
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS realm_domain (
                domain TEXT PRIMARY KEY,
                keys_json TEXT NOT NULL,
                pins_json TEXT NOT NULL,
                pinned_at INTEGER NOT NULL,
                checked_at INTEGER NOT NULL
            )",
        )
        .execute(&mut *tx)
        .await?;

        // Realm issuer keys this Actr Facility Provider holds in the Signer. `key_id`
        // is the Signer key id, the credential `key_id` and, in decimal, the
        // Realm Profile issuer key id. `next` is listed in the Realm Profile
        // but does not sign yet, `current` signs, `retired` only verifies
        // until `remove_after`. At most one `next` and one `current`.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS realm_issuer_key (
                realm TEXT NOT NULL,
                key_id INTEGER NOT NULL,
                signer_name TEXT NOT NULL UNIQUE,
                public_key TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('next', 'current', 'retired')),
                created_at INTEGER NOT NULL,
                published_at INTEGER,
                current_since INTEGER,
                remove_after INTEGER,
                PRIMARY KEY(realm, key_id),
                FOREIGN KEY(realm) REFERENCES realm(name) ON DELETE CASCADE
            )",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_realm_issuer_key_state
             ON realm_issuer_key(realm, state) WHERE state != 'retired'",
        )
        .execute(&mut *tx)
        .await?;

        // Latest accepted Realm Profile of each Realm, stored verbatim.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS realm_profile (
                realm TEXT PRIMARY KEY,
                sequence INTEGER NOT NULL,
                digest TEXT NOT NULL,
                document TEXT NOT NULL,
                accepted_at INTEGER NOT NULL,
                FOREIGN KEY(realm) REFERENCES realm(name) ON DELETE CASCADE
            )",
        )
        .execute(&mut *tx)
        .await?;

        // Realm members (`actr/protocols/realm-membership.md`). At most one
        // active owner per Realm.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS realm_member (
                realm TEXT NOT NULL REFERENCES realm(name) ON DELETE CASCADE,
                member_id TEXT NOT NULL,
                role TEXT NOT NULL CHECK(role IN ('owner', 'editor', 'viewer')),
                status TEXT NOT NULL CHECK(status IN ('active', 'removed')),
                created_at INTEGER NOT NULL,
                removed_at INTEGER,
                PRIMARY KEY(realm, member_id)
            )",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS realm_member_one_owner
             ON realm_member(realm) WHERE role = 'owner' AND status = 'active'",
        )
        .execute(&mut *tx)
        .await?;

        // Realm client keys: one Ed25519 key per client of a member.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS realm_client_key (
                realm TEXT NOT NULL,
                key_id TEXT NOT NULL,
                public_key BLOB NOT NULL,
                member_id TEXT NOT NULL,
                label TEXT NOT NULL DEFAULT '',
                registered_at INTEGER NOT NULL,
                last_used_at INTEGER,
                revoked_at INTEGER,
                PRIMARY KEY(realm, key_id),
                UNIQUE(realm, public_key),
                FOREIGN KEY(realm, member_id) REFERENCES realm_member(realm, member_id)
                    ON DELETE CASCADE
            )",
        )
        .execute(&mut *tx)
        .await?;

        // Realm invites; only the SHA-256 of the secret is kept. `role` for
        // a new member, or `member_id` for a new client of a member.
        // `created_by` is the signing key id, or `holder` / `operator`.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS realm_invite (
                realm TEXT NOT NULL REFERENCES realm(name) ON DELETE CASCADE,
                invite_id TEXT NOT NULL,
                secret_hash TEXT NOT NULL UNIQUE,
                role TEXT CHECK(role IN ('owner', 'editor', 'viewer')),
                member_id TEXT,
                created_by TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                used_at INTEGER,
                used_by_key TEXT,
                revoked_at INTEGER,
                PRIMARY KEY(realm, invite_id),
                CHECK((role IS NULL) != (member_id IS NULL))
            )",
        )
        .execute(&mut *tx)
        .await?;

        // Realm management request nonces, kept until the request expires.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS realm_mgmt_nonce (
                nonce TEXT PRIMARY KEY,
                expires_at INTEGER NOT NULL
            )",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_realm_mgmt_nonce_expires
             ON realm_mgmt_nonce(expires_at)",
        )
        .execute(&mut *tx)
        .await?;

        // Create the access control list table.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS actoracl (
                rowid INTEGER PRIMARY KEY AUTOINCREMENT,
                realm TEXT NOT NULL,
                source_realm TEXT,
                from_type TEXT NOT NULL,
                to_type TEXT NOT NULL,
                access INTEGER NOT NULL CHECK(access IN (0, 1)),
                FOREIGN KEY(realm) REFERENCES realm(name) ON DELETE CASCADE
            )",
        )
        .execute(&mut *tx)
        .await?;

        // Create indexes.
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_actoracl_realm
             ON actoracl(realm)",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_actoracl_lookup
             ON actoracl(realm, source_realm, from_type, to_type)",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_actoracl_policy
             ON actoracl(
                realm,
                COALESCE(source_realm, ''),
                from_type,
                to_type
             )",
        )
        .execute(&mut *tx)
        .await?;

        // Pending registration data: Issuer writes, signaling reads on WS connect
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_registration (
                realm TEXT NOT NULL,
                serial_number INTEGER NOT NULL,
                service_spec_blob BLOB,
                ws_address TEXT,
                created_at INTEGER NOT NULL,
                expires_at INTEGER,
                PRIMARY KEY(realm, serial_number),
                FOREIGN KEY(realm) REFERENCES realm(name) ON DELETE CASCADE
            )",
        )
        .execute(&mut *tx)
        .await?;

        // Signaling reads pending rows with `expires_at IS NULL OR expires_at >
        // unixepoch()`; index the column so the expiry filter stays cheap.
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_pending_registration_expiry \
             ON pending_registration(expires_at)",
        )
        .execute(&mut *tx)
        .await?;

        // Permanent Actor instance identity ledger. Snowflake worker leases
        // provide distributed uniqueness; this table additionally makes local
        // uniqueness fail closed across process restarts and clock rollback.
        // `target` persists the package target bound at registration so that
        // /renew and /reissue can recheck the exact (type_str, target) package
        // row instead of masking a revoked target behind a sibling that is still
        // active.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS issued_actor_identity (
                realm TEXT NOT NULL,
                serial_number INTEGER NOT NULL,
                actor_type TEXT NOT NULL,
                target TEXT,
                auth_mode INTEGER,
                development INTEGER NOT NULL DEFAULT 0,
                member_id TEXT NOT NULL,
                client_key_id TEXT NOT NULL,
                issued_at INTEGER NOT NULL,
                PRIMARY KEY(realm, serial_number),
                FOREIGN KEY(realm) REFERENCES realm(name) ON DELETE CASCADE
            )",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_issued_actor_identity_client_key
             ON issued_actor_identity(realm, client_key_id)",
        )
        .execute(&mut *tx)
        .await?;

        // Publisher Registry tables
        // name is the canonical HTTPS publisher domain.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS publisher (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                name         TEXT    NOT NULL UNIQUE,
                public_key   TEXT    NOT NULL DEFAULT '',
                key_id       TEXT    NOT NULL DEFAULT '',
                profile_json TEXT    NOT NULL DEFAULT '{}',
                contact      TEXT,
                status       TEXT    NOT NULL DEFAULT 'pending',
                created_at   INTEGER NOT NULL,
                updated_at   INTEGER,
                verified_at  INTEGER,
                suspended_at INTEGER,
                revoked_at   INTEGER,
                key_expires_at  INTEGER,
                next_public_key TEXT,
                next_key_id  TEXT,
                next_key_expires_at INTEGER,
                profile_pins TEXT
            )",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS publisher_package (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                publisher_id       INTEGER NOT NULL REFERENCES publisher(id),
                publisher TEXT    NOT NULL,
                name         TEXT    NOT NULL,
                version      TEXT    NOT NULL,
                type_str     TEXT    NOT NULL,
                target       TEXT    NOT NULL,
                manifest     TEXT    NOT NULL,
                signature    TEXT    NOT NULL,
                signing_key_id TEXT,
                proto_files  TEXT,
                status       TEXT    NOT NULL DEFAULT 'active',
                published_at INTEGER NOT NULL,
                revoked_at   INTEGER,
                UNIQUE(publisher, name, version, target)
            )",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_publisher_package_type ON publisher_package(type_str)",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_publisher_package_publisher ON publisher_package(publisher_id, status)",
        )
        .execute(&mut *tx)
        .await?;

        // Publisher Registry key history: stores retired public keys for JWKS-style rotation
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS publisher_key_history (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                publisher_id       INTEGER NOT NULL REFERENCES publisher(id),
                key_id       TEXT    NOT NULL,
                public_key   TEXT    NOT NULL,
                status       TEXT    NOT NULL DEFAULT 'retired',
                created_at   INTEGER NOT NULL,
                retired_at   INTEGER NOT NULL
            )",
        )
        .execute(&mut *tx)
        .await?;

        // UNIQUE so a given (publisher_id, key_id) can be archived at most once. This
        // backs the application-layer blacklist that forbids reinstating a
        // previously used publisher key: even a code path that skipped the check
        // cannot create a second history row for the same key. Normal forward
        // rotation always archives a distinct outgoing key, so this never fires
        // on the healthy path.
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_publisher_key_history_lookup
             ON publisher_key_history(publisher_id, key_id)",
        )
        .execute(&mut *tx)
        .await?;

        // Publish nonce table for challenge-response authentication on
        // /registry/packages/publish.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS publisher_publish_nonce (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                publisher_id     INTEGER NOT NULL REFERENCES publisher(id),
                nonce      BLOB    NOT NULL UNIQUE,
                status     TEXT    NOT NULL DEFAULT 'pending',
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            )",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_publisher_publish_nonce_expires
             ON publisher_publish_nonce(expires_at)",
        )
        .execute(&mut *tx)
        .await?;

        // Issuer renewal token table — only stores SHA-256(token), never the raw token.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS issuer_renewal_token (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                actor_id TEXT NOT NULL,
                token_hash BLOB NOT NULL UNIQUE,
                expires_at INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_issuer_renewal_token_actor
             ON issuer_renewal_token(actor_id)",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_issuer_renewal_token_expires
             ON issuer_renewal_token(expires_at)",
        )
        .execute(&mut *tx)
        .await?;

        // Durable idempotency journal for Issuer identity-state writes
        // (`/renew`, `/reissue`, keyed by the client's attempt_id).
        // Response bodies contain bearer credentials and are therefore
        // encrypted by Issuer before being stored in response_ciphertext.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS issuer_control_attempt (
                operation           TEXT    NOT NULL,
                attempt_id          BLOB    NOT NULL,
                actor_id            TEXT    NOT NULL,
                request_hash        BLOB    NOT NULL,
                state               INTEGER NOT NULL,
                lease_id            BLOB,
                lease_expires_at    INTEGER,
                response_status     INTEGER,
                response_ciphertext BLOB,
                response_expires_at INTEGER,
                tombstone_expires_at INTEGER NOT NULL,
                created_at          INTEGER NOT NULL,
                updated_at          INTEGER NOT NULL,
                PRIMARY KEY(operation, attempt_id)
            )",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_issuer_control_attempt_expiry
             ON issuer_control_attempt(state, tombstone_expires_at)",
        )
        .execute(&mut *tx)
        .await?;

        // Explicit actor revocation list. A revoked (realm, serial_number)
        // can never be renewed or reissued again, independent of any renewal
        // token it may still hold: /renew and /reissue consult this table before
        // signing. The composite primary key makes revocation idempotent.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS revoked_actor (
                realm TEXT NOT NULL,
                serial_number INTEGER NOT NULL,
                revoked_at INTEGER NOT NULL,
                reason TEXT,
                PRIMARY KEY(realm, serial_number),
                FOREIGN KEY(realm) REFERENCES realm(name) ON DELETE CASCADE
            )",
        )
        .execute(&mut *tx)
        .await?;

        sqlx::QueryBuilder::<sqlx::Sqlite>::new("PRAGMA user_version = ")
            .push(LORAN_SCHEMA_VERSION)
            .build()
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(())
    }

    /// Get the database connection pool
    pub fn get_pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Execute an SQL statement and return the number of affected rows
    pub async fn execute(&self, sql: impl sqlx::SqlSafeStr) -> Result<u64> {
        let result = sqlx::query(sql).execute(&self.pool).await?;
        Ok(result.rows_affected())
    }
}

use tokio::sync::OnceCell;

/// Global database instance
static GLOBAL_DATABASE: OnceCell<Database> = OnceCell::const_new();

/// Set the global database path
pub async fn set_db_path(path: &Path) -> Result<()> {
    let database = Database::new(path).await?;
    GLOBAL_DATABASE
        .set(database)
        .map_err(|_| anyhow::anyhow!("Database already initialized"))?;
    Ok(())
}

/// Get the global database instance
pub fn get_database() -> &'static Database {
    GLOBAL_DATABASE
        .get()
        .expect("Database not initialized. Call set_db_path first.")
}

/// Check whether the database has been initialized
pub fn is_database_initialized() -> bool {
    GLOBAL_DATABASE.get().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_database_is_versioned_and_hosts_many_realms() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path()).await.unwrap();
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(db.get_pool())
            .await
            .unwrap();
        assert_eq!(version, LORAN_SCHEMA_VERSION);

        for name in ["primary@example.com", "second@example.com"] {
            sqlx::query("INSERT INTO realm (name, created_at) VALUES (?, 1)")
                .bind(name)
                .execute(db.get_pool())
                .await
                .unwrap();
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM realm")
            .fetch_one(db.get_pool())
            .await
            .unwrap();
        assert_eq!(count, 2);

        let unknown_status = sqlx::query(
            "INSERT INTO realm (name, status, created_at)
             VALUES ('third@example.com', 'suspended', 1)",
        )
        .execute(db.get_pool())
        .await;
        assert!(unknown_status.is_err());
    }

    #[tokio::test]
    async fn realm_name_is_immutable_and_retirement_is_final() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path()).await.unwrap();
        sqlx::query("INSERT INTO realm (name, created_at) VALUES ('primary@example.com', 1)")
            .execute(db.get_pool())
            .await
            .unwrap();

        let rename = sqlx::query("UPDATE realm SET name = 'renamed@example.com'")
            .execute(db.get_pool())
            .await;
        assert!(rename.is_err(), "realm name must be immutable");

        sqlx::query("UPDATE realm SET status = 'retired'")
            .execute(db.get_pool())
            .await
            .unwrap();
        let revive = sqlx::query("UPDATE realm SET status = 'active'")
            .execute(db.get_pool())
            .await;
        assert!(revive.is_err(), "retired realm must stay retired");
        let reuse =
            sqlx::query("INSERT INTO realm (name, created_at) VALUES ('primary@example.com', 2)")
                .execute(db.get_pool())
                .await;
        assert!(reuse.is_err(), "retired realm name must not be reused");
    }

    #[tokio::test]
    async fn realm_name_is_the_key_of_realm_scoped_tables() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path()).await.unwrap();

        let unnamed = sqlx::query(
            "INSERT INTO realm (name, status, enabled, created_at)
             VALUES ('primary', 'active', 1, 1)",
        )
        .execute(db.get_pool())
        .await;
        assert!(unnamed.is_err(), "realm name must be <realm_id>@<domain>");

        sqlx::query(
            "INSERT INTO realm (name, status, enabled, created_at)
             VALUES ('primary@example.com', 'active', 1, 1)",
        )
        .execute(db.get_pool())
        .await
        .unwrap();
        for table in [
            "issued_actor_identity",
            "pending_registration",
            "revoked_actor",
        ] {
            let pk: Vec<String> = sqlx::query_scalar(
                "SELECT name FROM pragma_table_info(?) WHERE pk > 0 ORDER BY pk",
            )
            .bind(table)
            .fetch_all(db.get_pool())
            .await
            .unwrap();
            assert_eq!(pk, ["realm", "serial_number"], "{table}");
        }
        let orphan = sqlx::query(
            "INSERT INTO revoked_actor (realm, serial_number, revoked_at)
             VALUES ('other@example.com', 1, 1)",
        )
        .execute(db.get_pool())
        .await;
        assert!(orphan.is_err(), "rows must reference an existing Realm");
    }

    #[tokio::test]
    async fn unversioned_existing_database_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let db_file = dir.path().join("loran.db");
        let options = SqliteConnectOptions::from_str(&format!("sqlite:{}", db_file.display()))
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE realm (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let error = Database::new(dir.path())
            .await
            .err()
            .expect("legacy database must be rejected");
        let message = error.to_string();
        assert!(message.contains("schema version 0"));
        assert!(message.contains("rebuild the database"));
    }

    #[tokio::test]
    async fn previous_schema_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path()).await.unwrap();
        sqlx::QueryBuilder::<sqlx::Sqlite>::new("PRAGMA user_version = ")
            .push(LORAN_SCHEMA_VERSION - 1)
            .build()
            .execute(db.get_pool())
            .await
            .unwrap();
        db.get_pool().close().await;

        let error = Database::new(dir.path())
            .await
            .err()
            .expect("previous schema version must be rejected");
        let message = error.to_string();
        assert!(message.contains(&format!("schema version {}", LORAN_SCHEMA_VERSION - 1)));
        assert!(message.contains("rebuild the database"));
    }
}
