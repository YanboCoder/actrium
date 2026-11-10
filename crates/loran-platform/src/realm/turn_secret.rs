//! Per-Realm TURN secrets.
//!
//! `turn.turn_secret` is the Actr Facility Provider's master secret. Each Realm's
//! TURN credentials are keyed by
//! `HKDF-SHA256(master, salt = "actr.turn", info = "<realm>|<provider>")`, so
//! a credential minted for one Realm never authenticates in another.

use hkdf::Hkdf;
use sha2::Sha256;

const SALT: &[u8] = b"actr.turn";

/// The TURN HMAC key of `realm` under Actr Facility Provider `provider`.
pub fn realm_turn_secret(master: &[u8], realm: &str, provider: &str) -> [u8; 32] {
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(SALT), master)
        .expand(format!("{realm}|{provider}").as_bytes(), &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn each_realm_and_provider_gets_its_own_secret() {
        let a = realm_turn_secret(MASTER, "a@example.com", "provider.example");
        assert_eq!(
            a,
            realm_turn_secret(MASTER, "a@example.com", "provider.example")
        );
        assert_ne!(
            a,
            realm_turn_secret(MASTER, "b@example.com", "provider.example")
        );
        assert_ne!(
            a,
            realm_turn_secret(MASTER, "a@example.com", "other.example")
        );
        assert_ne!(
            a,
            realm_turn_secret(
                b"another master secret of 32 bytes",
                "a@example.com",
                "provider.example"
            )
        );
    }
}
