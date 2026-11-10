//! TURN authenticator
//!
//! Implements TURN server authentication and authorization based on HMAC-SHA1 time-limited credentials (coturn --use-auth-secret compatible format).
//!
//! # Authentication Flow
//!
//! 1. Issuer generates a Realm/Actr Facility Provider/audience/Actor scoped username, and uses
//!    `base64(HMAC-SHA1(realm_turn_secret, username))` as the password, where
//!    the Realm's secret is derived from the master `turn_secret` with HKDF
//!    (`platform::realm::realm_turn_secret`).
//! 2. The client authenticates with the TURN server using (username, password)
//! 3. The TURN server computes MD5(username:realm:password) as the long-term credential key
//! 4. turn_crate uses this key to verify the HMAC-SHA1 message integrity in requests

use crate::inactive_realms::InactiveRealms;
use base64::prelude::*;
use hmac::{Hmac, Mac};
use lru::LruCache;
use sha1::Sha1;
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use turn_crate::Error;
use turn_crate::auth::AuthHandler;

type HmacSha1 = Hmac<Sha1>;

const AUTH_CACHE_CAPACITY: usize = 4096;

// --- Admission rate limiting (TRN-02) ----------------------------------------
//
// Upstream `turn` decouples auth (`auth_handle`, which sees the source IP and
// the actor-scoped username) from allocation (`allocate_conn`, which sees only
// a port). Because every allocation/permission-driving request must first pass
// `auth_handle`, throttling here is the only per-source / per-actor admission
// point reachable without forking the upstream crate. It cannot bound the exact
// number of live allocations, permissions, or channels — those live in the
// allocation manager, which has no actor/IP context (see the module residual
// notes).
//
// The buckets below are token buckets keyed by source IP and by actor scope.
// Each is stored in an LRU cache with a hard capacity so a flood of distinct
// sources/actors can never grow memory without bound: once the cap is reached,
// inserting a new key evicts the least-recently-used entry. Evicting an idle
// entry only refunds that key its full burst on next contact, which is exactly
// the behaviour we want for a long-idle source.

/// Hard cap on distinct source IPs tracked for admission. Bounds memory at
/// roughly `capacity * size_of::<(IpAddr, TokenBucket)>()`; LRU eviction
/// reclaims the least-recently-seen source when the cap is hit.
const ADMISSION_IP_CAPACITY: usize = 65_536;

/// Hard cap on distinct actor scopes tracked for admission. Same LRU bound.
const ADMISSION_ACTOR_CAPACITY: usize = 65_536;

/// Per-source-IP burst: a single source may issue this many auth requests
/// back-to-back before refill governs it. Chosen generously so a legitimate
/// client (one allocation plus periodic refreshes/permissions) is never
/// throttled; a single client in normal operation stays far below it.
const ADMISSION_IP_BURST: f64 = 128.0;

/// Per-source-IP sustained refill rate (tokens per second). A source sustaining
/// more than this many auth requests per second is throttled.
const ADMISSION_IP_REFILL_PER_SEC: f64 = 32.0;

/// Per-actor burst. Looser than per-IP because one actor legitimately connects
/// from multiple clients/networks; this is a soft safety cap, not the primary
/// control.
const ADMISSION_ACTOR_BURST: f64 = 256.0;

/// Per-actor sustained refill rate (tokens per second).
const ADMISSION_ACTOR_REFILL_PER_SEC: f64 = 64.0;

/// A single token bucket. Refills continuously at `refill_per_sec` up to
/// `burst`, and admits a request by consuming one token.
#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    /// A fresh bucket starts full so a previously-unseen key is admitted
    /// immediately (and enjoys its full burst).
    fn full(burst: f64, now: Instant) -> Self {
        Self {
            tokens: burst,
            last_refill: now,
        }
    }

    /// Refill according to elapsed time, then try to consume one token.
    /// Returns true if the request is admitted.
    fn try_admit(&mut self, burst: f64, refill_per_sec: f64, now: Instant) -> bool {
        let elapsed = now
            .saturating_duration_since(self.last_refill)
            .as_secs_f64();
        self.last_refill = now;
        self.tokens = (self.tokens + elapsed * refill_per_sec).min(burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Per-source-IP and per-actor admission token buckets. Both maps are bounded
/// LRU caches, so distinct-source / distinct-actor floods cannot grow memory
/// without bound.
struct AdmissionControl {
    per_ip: Mutex<LruCache<IpAddr, TokenBucket>>,
    per_actor: Mutex<LruCache<String, TokenBucket>>,
}

impl AdmissionControl {
    fn new() -> Self {
        let ip_cap = NonZeroUsize::new(ADMISSION_IP_CAPACITY)
            .expect("ADMISSION_IP_CAPACITY must be non-zero");
        let actor_cap = NonZeroUsize::new(ADMISSION_ACTOR_CAPACITY)
            .expect("ADMISSION_ACTOR_CAPACITY must be non-zero");
        Self {
            per_ip: Mutex::new(LruCache::new(ip_cap)),
            per_actor: Mutex::new(LruCache::new(actor_cap)),
        }
    }

    fn admit_ip(&self, ip: IpAddr, now: Instant) -> bool {
        let mut cache = self.per_ip.lock().expect("admission ip cache poisoned");
        if let Some(bucket) = cache.get_mut(&ip) {
            bucket.try_admit(ADMISSION_IP_BURST, ADMISSION_IP_REFILL_PER_SEC, now)
        } else {
            let mut bucket = TokenBucket::full(ADMISSION_IP_BURST, now);
            let admitted = bucket.try_admit(ADMISSION_IP_BURST, ADMISSION_IP_REFILL_PER_SEC, now);
            cache.put(ip, bucket);
            admitted
        }
    }

    fn admit_actor(&self, actor_scope: &str, now: Instant) -> bool {
        let mut cache = self
            .per_actor
            .lock()
            .expect("admission actor cache poisoned");
        if let Some(bucket) = cache.get_mut(actor_scope) {
            bucket.try_admit(ADMISSION_ACTOR_BURST, ADMISSION_ACTOR_REFILL_PER_SEC, now)
        } else {
            let mut bucket = TokenBucket::full(ADMISSION_ACTOR_BURST, now);
            let admitted =
                bucket.try_admit(ADMISSION_ACTOR_BURST, ADMISSION_ACTOR_REFILL_PER_SEC, now);
            cache.put(actor_scope.to_string(), bucket);
            admitted
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AuthCacheKey {
    username: String,
    realm: String,
}

#[derive(Debug, Clone)]
struct CachedAuthKey {
    integrity_key: Vec<u8>,
    expires_at: u64,
}

/// TURN authenticator (HMAC time-limited credential mode).
pub struct Authenticator {
    turn_secret: String,
    /// Actr Facility Provider ID that credential usernames must carry.
    provider_id: String,
    auth_key_cache: Mutex<LruCache<AuthCacheKey, CachedAuthKey>>,
    admission: AdmissionControl,
    /// Realms whose credentials are refused
    inactive_realms: InactiveRealms,
}

impl Authenticator {
    pub fn new(turn_secret: String, provider_id: String) -> Result<Self, Error> {
        if turn_secret.len() < 32 {
            return Err(Error::Other(
                "TURN secret must be at least 32 bytes".to_string(),
            ));
        }
        platform::recording::info!(
            "TURN authenticator initialized (HMAC time-limited credential mode, LRU cache)"
        );
        let capacity =
            NonZeroUsize::new(AUTH_CACHE_CAPACITY).expect("AUTH_CACHE_CAPACITY must be non-zero");
        Ok(Self {
            turn_secret,
            provider_id,
            auth_key_cache: Mutex::new(LruCache::new(capacity)),
            admission: AdmissionControl::new(),
            inactive_realms: InactiveRealms::default(),
        })
    }

    /// Refuse credentials of the Realms in `inactive_realms`
    pub fn with_inactive_realms(mut self, inactive_realms: InactiveRealms) -> Self {
        self.inactive_realms = inactive_realms;
        self
    }

    /// Get cache statistics (for monitoring and debugging)
    pub fn cache_stats(&self) -> (usize, usize) {
        let cache = self.auth_key_cache.lock().expect("auth cache poisoned");
        (cache.len(), cache.cap().get())
    }

    /// Clear the cache (for testing or manual reset)
    #[allow(dead_code)]
    pub fn clear_cache(&self) {
        let mut cache = self.auth_key_cache.lock().expect("auth cache poisoned");
        cache.clear();
        platform::recording::info!("TURN auth key cache cleared");
    }

    /// Compute the HMAC-SHA1 password (base64-encoded)
    ///
    /// `password = base64(HMAC-SHA1(realm_turn_secret, username))`
    fn compute_password(&self, realm: &str, username: &str) -> String {
        let secret = platform::realm::realm_turn_secret(
            self.turn_secret.as_bytes(),
            realm,
            &self.provider_id,
        );
        let mut mac = HmacSha1::new_from_slice(&secret).expect("HMAC-SHA1 accepts any key length");
        mac.update(username.as_bytes());
        BASE64_STANDARD.encode(mac.finalize().into_bytes())
    }

    fn auth_handle_at(
        &self,
        username: &str,
        server_realm: &str,
        src_addr: SocketAddr,
        now: u64,
    ) -> Result<Vec<u8>, Error> {
        platform::recording::debug!(
            "Processing TURN auth: realm={}, src={}",
            server_realm,
            src_addr
        );

        // Always parse and validate the expiry before consulting the cache. A cached
        // integrity key must never extend the lifetime of a time-limited credential.
        let (expires_at_str, scoped_identity) = username.split_once(':').ok_or_else(|| {
            platform::recording::warn!("TURN credential username has invalid structure");
            Error::Other("invalid username format".to_string())
        })?;

        let expires_at: u64 = expires_at_str.parse().map_err(|e| {
            platform::recording::warn!("TURN credential expiry is invalid: error={}", e);
            Error::Other(format!("invalid expires_at in username: {e}"))
        })?;

        if expires_at <= now {
            platform::recording::warn!(
                "TURN credential expired: expires_at={}, now={}",
                expires_at,
                now
            );
            return Err(Error::Other("TURN credential expired".to_string()));
        }

        let scoped_identity = scoped_identity
            .strip_prefix("realm=")
            .ok_or_else(|| Error::Other("invalid TURN credential scope".to_string()))?;
        let (realm, scoped_identity) = scoped_identity
            .split_once(":provider=")
            .ok_or_else(|| Error::Other("invalid TURN credential scope".to_string()))?;
        let (provider, scoped_identity) = scoped_identity
            .split_once(":audience=")
            .ok_or_else(|| Error::Other("invalid TURN credential scope".to_string()))?;
        let (audience, scoped_identity) = scoped_identity
            .split_once(":client_key=")
            .ok_or_else(|| Error::Other("missing TURN client key binding".to_string()))?;
        let (client_key, actor_id) = scoped_identity
            .split_once(":actor=")
            .ok_or_else(|| Error::Other("invalid TURN credential scope".to_string()))?;
        let actor = actr_protocol::ActrId::from_string_repr(actor_id)
            .map_err(|_| Error::Other("invalid TURN credential Actor ID".to_string()))?;
        if realm != actor.realm || provider != self.provider_id || audience != "actr.turn" {
            return Err(Error::Other(
                "TURN credential scope does not match this Realm/Actr Facility Provider"
                    .to_string(),
            ));
        }
        // Checked before the cache, so a cached key never outlives the Realm
        // or the Actor's Realm client key.
        if self.inactive_realms.contains(realm) {
            platform::recording::warn!("TURN credential refused: Realm {} is not active", realm);
            return Err(Error::Other(
                "TURN credential Realm is not active".to_string(),
            ));
        }
        if self.inactive_realms.contains_actor(actor_id) {
            platform::recording::warn!("TURN credential refused: Actor {} is disabled", actor_id);
            return Err(Error::Other(
                "TURN credential Actor is disabled".to_string(),
            ));
        }
        if !self.inactive_realms.matches_binding(actor_id, client_key) {
            return Err(Error::Other(
                "TURN credential client key binding is no longer current".to_string(),
            ));
        }

        let cache_key = AuthCacheKey {
            username: username.to_string(),
            realm: server_realm.to_string(),
        };
        if let Some(cached) = self
            .auth_key_cache
            .lock()
            .expect("auth cache poisoned")
            .get(&cache_key)
            .cloned()
            .filter(|entry| entry.expires_at > now)
        {
            platform::recording::debug!("TURN authentication cache hit");
            return Ok(cached.integrity_key);
        }

        let password = self.compute_password(realm, username);

        // Per-actor admission on the cache-miss path. NOTE: this is a throttle,
        // not an authentication gate — `compute_password` is a server-side HMAC
        // that always succeeds and does not prove the requester holds the
        // secret; the username is only authenticated when the upstream turn
        // crate verifies MESSAGE-INTEGRITY with the key returned below (after
        // this function returns). The `auth_key_cache` only dedupes *identical*
        // (username, realm) pairs — a legitimate client repeating its username
        // skips admission. It does NOT bound a forged-username drain: usernames
        // embed attacker-controlled fields (`expires_at` and friends), so each
        // variation is a fresh cache key that lands here. The actual bound on
        // that drain is per-IP admission (each source capped at
        // ADMISSION_IP_BURST/REFILL) plus the actor bucket itself — exhausting
        // the victim's bucket throttles the victim's TURN auths until refill,
        // a degradation, not an auth bypass.
        self.enforce_admission_actor(username, Instant::now())?;

        // Compute long-term credential key: MD5(username:realm:password).
        // This construction is required by the TURN long-term credential protocol.
        let integrity_text = format!("{username}:{server_realm}:{password}");
        let digest = md5::compute(integrity_text.as_bytes());
        let result = digest.to_vec();

        self.auth_key_cache
            .lock()
            .expect("auth cache poisoned")
            .put(
                cache_key,
                CachedAuthKey {
                    integrity_key: result.clone(),
                    expires_at,
                },
            );

        platform::recording::debug!(
            "TURN auth succeeded: actor_id={}, expires_at={}, cache_size={}/{}",
            actor_id,
            expires_at,
            self.cache_stats().0,
            self.cache_stats().1
        );

        Ok(result)
    }

    /// Extract the actor scope substring from a scoped username, if present.
    /// Used only as a stable admission key; malformed usernames are rejected
    /// later by `auth_handle_at` regardless.
    fn actor_scope_key(username: &str) -> Option<&str> {
        username.split_once(":actor=").map(|(_, actor)| actor)
    }

    /// Admission gate applied before any allocation/permission-driving auth.
    ///
    /// Throttles per source IP (primary) and per actor scope (soft cap). Both
    /// buckets are bounded LRU maps, so this cannot itself grow memory without
    /// bound. Returns an error when the source or actor exceeds its rate, which
    /// makes `auth_handle` reject the request and denies the flood.
    #[cfg(test)]
    fn enforce_admission(&self, username: &str, src_ip: IpAddr, now: Instant) -> Result<(), Error> {
        self.enforce_admission_ip(src_ip, now)?;
        self.enforce_admission_actor(username, now)?;
        Ok(())
    }

    /// Per-source-IP admission. Runs before any credential work because a single
    /// source IP flooding unauthenticated packets is the primary flood vector and
    /// does not depend on the (untrusted, pre-MESSAGE-INTEGRITY) username.
    fn enforce_admission_ip(&self, src_ip: IpAddr, now: Instant) -> Result<(), Error> {
        if !self.admission.admit_ip(src_ip, now) {
            platform::recording::warn!(
                "TURN admission throttled: source IP {} exceeded auth rate",
                src_ip
            );
            return Err(Error::Other(
                "TURN admission rate limit exceeded for source IP".to_string(),
            ));
        }
        Ok(())
    }

    /// Per-actor admission throttle. Called after `compute_password` on the
    /// cache-miss path. Note this is **not** an authentication gate:
    /// `compute_password` is a server-side HMAC that always succeeds, and the
    /// username is only authenticated once the upstream turn crate verifies
    /// MESSAGE-INTEGRITY (after `auth_handle_at` returns). An attacker who
    /// forges a victim's username charges the victim's actor bucket; varying
    /// username fields (`expires_at`, ...) defeats the auth-key cache, so the
    /// drain is bounded only by per-IP admission and the actor bucket itself
    /// (worst case: the victim's TURN auths are throttled until refill — a
    /// denial-of-service degradation, not an auth bypass). Per-IP admission
    /// (`enforce_admission_ip`) is the primary gate against unauthenticated
    /// floods.
    fn enforce_admission_actor(&self, username: &str, now: Instant) -> Result<(), Error> {
        if let Some(actor_scope) = Self::actor_scope_key(username)
            && !self.admission.admit_actor(actor_scope, now)
        {
            platform::recording::warn!("TURN admission throttled: actor exceeded auth rate");
            return Err(Error::Other(
                "TURN admission rate limit exceeded for actor".to_string(),
            ));
        }
        Ok(())
    }
}

impl AuthHandler for Authenticator {
    fn password_algorithms(&self) -> Vec<u16> {
        vec![2, 1]
    }

    fn auth_handle_algorithm(
        &self,
        username: &str,
        server_realm: &str,
        src_addr: SocketAddr,
        algorithm: u16,
    ) -> Result<Vec<u8>, Error> {
        let legacy = self.auth_handle(username, server_realm, src_addr)?;
        match algorithm {
            1 => Ok(legacy),
            2 => {
                let (_, scope) = username
                    .split_once(":realm=")
                    .ok_or_else(|| Error::Other("invalid TURN credential scope".into()))?;
                let (realm, _) = scope
                    .split_once(":provider=")
                    .ok_or_else(|| Error::Other("invalid TURN credential scope".into()))?;
                turn_crate::auth::generate_auth_key_algorithm(
                    username,
                    server_realm,
                    &self.compute_password(realm, username),
                    2,
                )
            }
            _ => Err(Error::Other("unsupported password algorithm".into())),
        }
    }
    fn auth_handle(
        &self,
        username: &str,
        server_realm: &str,
        src_addr: SocketAddr,
    ) -> Result<Vec<u8>, Error> {
        // Per-source-IP admission first: reject floods from a single source IP before
        // any credential work. Uses the monotonic clock so it is independent of the
        // wall-clock expiry checks below. The per-actor admission is deferred to
        // `auth_handle_at` (cache-miss path only) — see its comment for why that is a
        // throttle, not an authentication gate: the username is unauthenticated until
        // the upstream turn crate verifies MESSAGE-INTEGRITY.
        self.enforce_admission_ip(src_addr.ip(), Instant::now())?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.auth_handle_at(username, server_realm, src_addr, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::time::Duration;

    fn make_auth() -> Authenticator {
        Authenticator::new(
            "test-secret-at-least-32-bytes-long".to_string(),
            "provider.example".to_string(),
        )
        .expect("authenticator should initialize")
        .with_inactive_realms(active_bindings())
    }

    fn active_bindings() -> InactiveRealms {
        let state = InactiveRealms::default();
        for realm in ["k7m2q9xh@example.com", "p3n8w2rt@example.com"] {
            state.bind_actor(&format!("1.{realm}"), "aaaaaaaaaaaaaaaa");
        }
        state
    }

    fn username(expires_at: u64) -> String {
        format!(
            "{expires_at}:realm=k7m2q9xh@example.com:provider=provider.example:audience=actr.turn:client_key=aaaaaaaaaaaaaaaa:actor=1.k7m2q9xh@example.com"
        )
    }

    #[test]
    fn test_authenticator_creation() {
        let _auth = make_auth();
        assert!(Authenticator::new("short".to_string(), "provider.example".to_string()).is_err());
    }

    #[test]
    fn old_device_turn_credentials_stay_revoked_after_actor_recovery() {
        let auth = make_auth();
        let actor = "1.k7m2q9xh@example.com";
        let source = "127.0.0.1:3478".parse().unwrap();
        let old = username(2_000_000_060);
        let old_integrity = auth
            .auth_handle_at(&old, "loran.local", source, 2_000_000_000)
            .unwrap();
        auth.inactive_realms.insert_actor(actor);
        assert!(
            auth.auth_handle_at(&old, "loran.local", source, 2_000_000_000)
                .is_err()
        );
        auth.inactive_realms.bind_actor(actor, "bbbbbbbbbbbbbbbb");
        auth.inactive_realms.actors_for_test_resume(actor);
        // The old integrity key is still cached, but its binding is refused.
        assert!(
            auth.auth_handle_at(&old, "loran.local", source, 2_000_000_000)
                .is_err()
        );
        auth.clear_cache();
        assert!(
            auth.auth_handle_at(&old, "loran.local", source, 2_000_000_000)
                .is_err()
        );
        let new = old.replace("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb");
        let new_integrity = auth
            .auth_handle_at(&new, "loran.local", source, 2_000_000_000)
            .unwrap();
        assert_ne!(
            new_integrity, old_integrity,
            "altering the binding cannot reuse the old password"
        );
        let legacy = old.replace(":client_key=aaaaaaaaaaaaaaaa", "");
        assert!(
            auth.auth_handle_at(&legacy, "loran.local", source, 2_000_000_000)
                .is_err()
        );
    }

    #[test]
    fn test_cache_key_computation() {
        let key1 = AuthCacheKey {
            username: "user1".to_string(),
            realm: "realm1".to_string(),
        };
        let key2 = key1.clone();
        let key3 = AuthCacheKey {
            username: "user2".to_string(),
            realm: "realm1".to_string(),
        };
        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_cache_stats() {
        let auth = make_auth();
        let (size, capacity) = auth.cache_stats();
        assert_eq!(size, 0);
        assert_eq!(capacity, AUTH_CACHE_CAPACITY);
    }

    #[test]
    fn test_md5_computation() {
        let integrity_text = "testuser:testrealm:testpass";
        let result = md5::compute(integrity_text.as_bytes());
        assert_eq!(result.len(), 16);
        let result2 = md5::compute(integrity_text.as_bytes());
        assert_eq!(result.to_vec(), result2.to_vec());
    }

    #[test]
    fn test_auth_handle_rejects_invalid_username_format() {
        let auth = make_auth();
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");

        let err = auth
            .auth_handle("invalid-no-colon", "loran.local", src_addr)
            .expect_err("invalid username should be rejected");

        assert!(
            err.to_string().contains("invalid username format"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_auth_handle_rejects_expired_credential() {
        let auth = make_auth();
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");

        // Expired username (expires_at is in the past)
        let username = username(1_000_000_000);
        let err = auth
            .auth_handle(&username, "loran.local", src_addr)
            .expect_err("expired credential should be rejected");

        assert!(
            err.to_string().contains("expired"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_auth_handle_accepts_valid_credential() {
        let auth = make_auth();
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");

        // Non-expired credential
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let expires_at = now + 3600;
        let username = username(expires_at);

        let result = auth.auth_handle(&username, "loran.local", src_addr);
        assert!(result.is_ok(), "valid credential should succeed");
        assert_eq!(result.unwrap().len(), 16, "should return 16-byte MD5 key");
    }

    #[test]
    fn test_auth_handle_refuses_inactive_realm_even_when_cached() {
        let inactive = active_bindings();
        let auth = make_auth().with_inactive_realms(inactive.clone());
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");
        let username = username(2_000_000_060);

        auth.auth_handle_at(&username, "loran.local", src_addr, 2_000_000_000)
            .expect("active Realm authenticates");
        inactive.insert("k7m2q9xh@example.com");
        let error = auth
            .auth_handle_at(&username, "loran.local", src_addr, 2_000_000_000)
            .expect_err("inactive Realm is refused");
        assert!(error.to_string().contains("not active"), "{error}");
    }

    #[test]
    fn test_auth_handle_refuses_disabled_actor_even_when_cached() {
        let inactive = active_bindings();
        let auth = make_auth().with_inactive_realms(inactive.clone());
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");
        let username = username(2_000_000_060);
        let actor = username.split_once(":actor=").unwrap().1.to_string();

        auth.auth_handle_at(&username, "loran.local", src_addr, 2_000_000_000)
            .expect("enabled Actor authenticates");
        inactive.insert_actor(&actor);
        let error = auth
            .auth_handle_at(&username, "loran.local", src_addr, 2_000_000_000)
            .expect_err("disabled Actor is refused");
        assert!(error.to_string().contains("disabled"), "{error}");
    }

    #[test]
    fn test_auth_handle_rejects_cross_realm_or_audience_scope() {
        let auth = make_auth();
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");
        let base = "2000000060:realm=k7m2q9xh@example.com:provider=provider.example:audience=actr.turn:client_key=aaaaaaaaaaaaaaaa:actor=1.k7m2q9xh@example.com";

        for invalid in [
            base.replace("realm=k7m2q9xh@", "realm=other@"),
            base.replace("audience=actr.turn", "audience=actr.signaling"),
            base.replace("provider=provider.example", "provider=other"),
            base.replace("actor=1.k7m2q9xh@", "actor=1.other@"),
        ] {
            let error = auth
                .auth_handle_at(&invalid, "loran.local", src_addr, 2_000_000_000)
                .expect_err("mismatched credential scope must be rejected");
            assert!(error.to_string().contains("scope"));
        }
    }

    #[test]
    fn test_auth_handle_uses_cache() {
        let auth = make_auth();
        let now = 2_000_000_000;
        let username = username(2_000_000_060);
        let server_realm = "loran.local";
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");
        let expected_key = vec![0xAB; 16];

        let cache_key = AuthCacheKey {
            username: username.clone(),
            realm: server_realm.to_string(),
        };
        auth.auth_key_cache
            .lock()
            .expect("auth cache poisoned")
            .put(
                cache_key,
                CachedAuthKey {
                    integrity_key: expected_key.clone(),
                    expires_at: now + 60,
                },
            );

        let result = auth
            .auth_handle_at(&username, server_realm, src_addr, now)
            .expect("unexpired cached value should be used");

        assert_eq!(result, expected_key);
    }

    #[test]
    fn test_cached_auth_key_never_extends_credential_lifetime() {
        let auth = make_auth();
        let username = username(2_000_000_001);
        let server_realm = "loran.local";
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");

        auth.auth_handle_at(&username, server_realm, src_addr, 2_000_000_000)
            .expect("credential should initially be valid");
        assert_eq!(auth.cache_stats().0, 1);

        let error = auth
            .auth_handle_at(&username, server_realm, src_addr, 2_000_000_001)
            .expect_err("cached credential must be rejected at expiry");
        assert!(error.to_string().contains("expired"));
    }

    #[test]
    fn realm_credentials_are_not_interchangeable() {
        // Same serial, same Actor type, two Realms: each Realm's password is
        // keyed by its own derived secret.
        let auth = make_auth();
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");
        let a = username(2_000_000_060);
        let b = a.replace("k7m2q9xh@example.com", "p3n8w2rt@example.com");

        let a_key = auth
            .auth_handle_at(&a, "loran.local", src_addr, 2_000_000_000)
            .unwrap();
        let b_key = auth
            .auth_handle_at(&b, "loran.local", src_addr, 2_000_000_000)
            .unwrap();
        assert_ne!(a_key, b_key);

        // Realm A's secret does not produce Realm B's passwords.
        assert_ne!(
            auth.compute_password("k7m2q9xh@example.com", &b),
            auth.compute_password("p3n8w2rt@example.com", &b)
        );
    }

    #[test]
    fn test_auth_cache_is_scoped_to_secret_bearing_instance() {
        let first = Authenticator::new(
            "first-secret-at-least-32-bytes-long".to_string(),
            "provider.example".to_string(),
        )
        .unwrap()
        .with_inactive_realms(active_bindings());
        let second = Authenticator::new(
            "second-secret-at-least-32-bytes-long".to_string(),
            "provider.example".to_string(),
        )
        .unwrap()
        .with_inactive_realms(active_bindings());
        let username = username(2_000_000_060);
        let server_realm = "loran.local";
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");

        let first_key = first
            .auth_handle_at(&username, server_realm, src_addr, 2_000_000_000)
            .unwrap();
        let second_key = second
            .auth_handle_at(&username, server_realm, src_addr, 2_000_000_000)
            .unwrap();

        assert_ne!(first_key, second_key);
        assert_eq!(first.cache_stats().0, 1);
        assert_eq!(second.cache_stats().0, 1);
    }

    #[test]
    fn auth_cache_stays_bounded_under_unique_valid_credentials() {
        let auth = make_auth();
        let now = 2_000_000_000;
        let server_realm = "loran.local";
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");

        for serial in 1..=(AUTH_CACHE_CAPACITY + 64) {
            auth.inactive_realms.bind_actor(
                &format!("{serial:x}.k7m2q9xh@example.com"),
                "aaaaaaaaaaaaaaaa",
            );
            let username = format!(
                "{}:realm=k7m2q9xh@example.com:provider=provider.example:audience=actr.turn:client_key=aaaaaaaaaaaaaaaa:actor={:x}.k7m2q9xh@example.com",
                now + 60,
                serial
            );
            auth.auth_handle_at(&username, server_realm, src_addr, now)
                .expect("valid scoped credential should populate the bounded cache");
        }

        assert_eq!(
            auth.cache_stats(),
            (AUTH_CACHE_CAPACITY, AUTH_CACHE_CAPACITY)
        );
    }

    #[test]
    fn token_bucket_admits_burst_then_throttles_then_refills() {
        let start = Instant::now();
        let mut bucket = TokenBucket::full(ADMISSION_IP_BURST, start);

        // A full bucket admits exactly `burst` requests at a single instant.
        let mut admitted = 0usize;
        for _ in 0..(ADMISSION_IP_BURST as usize + 32) {
            if bucket.try_admit(ADMISSION_IP_BURST, ADMISSION_IP_REFILL_PER_SEC, start) {
                admitted += 1;
            }
        }
        assert_eq!(admitted, ADMISSION_IP_BURST as usize);

        // One second later, exactly `refill_per_sec` more requests are admitted.
        let later = start + Duration::from_secs(1);
        let mut refilled = 0usize;
        for _ in 0..(ADMISSION_IP_REFILL_PER_SEC as usize + 16) {
            if bucket.try_admit(ADMISSION_IP_BURST, ADMISSION_IP_REFILL_PER_SEC, later) {
                refilled += 1;
            }
        }
        assert_eq!(refilled, ADMISSION_IP_REFILL_PER_SEC as usize);
    }

    #[test]
    fn admission_throttles_single_source_ip_flood() {
        let control = AdmissionControl::new();
        let ip: IpAddr = "203.0.113.7".parse().expect("valid ip");
        let now = Instant::now();

        let mut admitted = 0usize;
        for _ in 0..(ADMISSION_IP_BURST as usize + 64) {
            if control.admit_ip(ip, now) {
                admitted += 1;
            }
        }
        assert_eq!(admitted, ADMISSION_IP_BURST as usize);
        // Once throttled, the same source stays throttled at the same instant.
        assert!(!control.admit_ip(ip, now));
    }

    #[test]
    fn admission_tracks_source_ips_independently() {
        let control = AdmissionControl::new();
        let now = Instant::now();
        let flooder: IpAddr = "203.0.113.7".parse().expect("valid ip");
        let bystander: IpAddr = "203.0.113.8".parse().expect("valid ip");

        // Exhaust the flooder's bucket.
        for _ in 0..(ADMISSION_IP_BURST as usize + 8) {
            control.admit_ip(flooder, now);
        }
        assert!(!control.admit_ip(flooder, now));
        // A different source is unaffected.
        assert!(control.admit_ip(bystander, now));
    }

    #[test]
    fn admission_throttles_single_actor_flood() {
        let control = AdmissionControl::new();
        let actor = "1.k7m2q9xh@example.com";
        let now = Instant::now();

        let mut admitted = 0usize;
        for _ in 0..(ADMISSION_ACTOR_BURST as usize + 64) {
            if control.admit_actor(actor, now) {
                admitted += 1;
            }
        }
        assert_eq!(admitted, ADMISSION_ACTOR_BURST as usize);
        assert!(!control.admit_actor(actor, now));
    }

    #[test]
    fn actor_scope_key_extracts_actor_or_none() {
        assert_eq!(
            Authenticator::actor_scope_key(&username(2_000_000_060)),
            Some("1.k7m2q9xh@example.com")
        );
        assert_eq!(Authenticator::actor_scope_key("no-actor-here"), None);
    }

    #[test]
    fn enforce_admission_allows_normal_client_then_denies_flood() {
        let auth = make_auth();
        let src_ip: IpAddr = "203.0.113.9".parse().expect("valid ip");
        let username = username(2_000_000_060);
        let now = Instant::now();

        // A normal client's handful of requests are always admitted.
        for _ in 0..8 {
            auth.enforce_admission(&username, src_ip, now)
                .expect("normal traffic must be admitted");
        }

        // A sustained flood from the same source eventually gets denied.
        let mut denied = false;
        for _ in 0..(ADMISSION_IP_BURST as usize + 64) {
            if auth.enforce_admission(&username, src_ip, now).is_err() {
                denied = true;
                break;
            }
        }
        assert!(denied, "sustained flood must be throttled");
    }

    #[test]
    fn auth_handle_admits_single_normal_request() {
        // The production trait entry applies admission; a single legitimate
        // request from loopback (as the existing tests use) must still pass.
        let auth = make_auth();
        let src_addr: SocketAddr = "127.0.0.1:3478".parse().expect("valid socket addr");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let username = username(now + 3600);

        let result = auth.auth_handle(&username, "loran.local", src_addr);
        assert!(result.is_ok(), "single normal request must be admitted");
    }
}
