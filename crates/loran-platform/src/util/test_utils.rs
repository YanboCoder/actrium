//! Test utilities module
//!
//! Provides test-related utility functions and helper functionality

#[cfg(test)]
pub mod utils {
    use std::path::Path;
    use tokio::sync::OnceCell;

    // Initialize the test database once globally
    static INIT: OnceCell<()> = OnceCell::const_new();

    // Setup test database
    pub async fn setup_test_db() -> anyhow::Result<()> {
        INIT.get_or_init(|| async {
            // Use a directory for the database, not a file directly.
            // nextest runs each test in its own process, so a fixed path is
            // concurrently deleted and rewritten (SQLITE_IOERR 1802). Suffix the
            // path with the PID so each process gets an isolated database; under
            // libtest the path stays shared within the single process, unchanged.
            let db_dir = std::env::temp_dir().join(format!("loran_test_db_{}", std::process::id()));

            // Ensure directory exists
            std::fs::create_dir_all(&db_dir).expect("Failed to create test database directory");

            // Remove existing database file if it exists
            let db_file = db_dir.join("loran.db");
            if db_file.exists() {
                let _ = std::fs::remove_file(&db_file);
            }

            let db_dir_str = db_dir
                .to_str()
                .expect("Failed to convert DB directory path to string");

            // Try to set database path, ignore error if already initialized
            match crate::storage::db::set_db_path(Path::new(db_dir_str)).await {
                Ok(()) => {}
                Err(e) => {
                    // If database is already initialized, that's okay for tests
                    let err_msg = e.to_string();
                    if !err_msg.contains("already initialized")
                        && !err_msg.contains("Database already")
                    {
                        panic!("Failed to initialize test database: {e}");
                    }
                }
            }
        })
        .await;

        // Tests share the process-global database, so reset application rows
        // while the `serial` test guard is held by callers.
        let pool = crate::storage::db::get_database().get_pool();
        let mut transaction = pool.begin().await?;
        for statement in [
            "DELETE FROM actoracl",
            "DELETE FROM pending_registration",
            "DELETE FROM issued_actor_identity",
            "DELETE FROM issuer_renewal_token",
            "DELETE FROM issuer_control_attempt",
            "DELETE FROM publisher_key_history",
            "DELETE FROM publisher",
            "DELETE FROM realm_issuer_key",
            "DELETE FROM realm_profile",
            "DELETE FROM realm_domain",
            "DELETE FROM realm_mgmt_nonce",
            "DELETE FROM realm_invite",
            "DELETE FROM realm_client_key",
            "DELETE FROM realm_member",
            "DELETE FROM realm",
        ] {
            sqlx::query(statement).execute(&mut *transaction).await?;
        }
        sqlx::query("DELETE FROM sqlite_sequence")
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }
}
