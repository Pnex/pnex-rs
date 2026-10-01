//! Short-TTL per-pod cache of the JIT-provisioned `users` row, keyed by the
//! IdP `sub`.
//!
//! Every authenticated request used to run the `users` lookup (plus the
//! drift check) before anything else. The cached row is reused only while:
//! - it is younger than the TTL (default 5 s, max 10 s — bounds how long a manual DB change
//!   such as a `platform_admin` revocation or a user deletion can lag on a
//!   pod), and
//! - the token's email / display name still match what was synced, so an
//!   IdP-side rename always goes through `sync_user` immediately.
//!
//! Deliberately NOT cached: membership / role / organization — they are
//! read fresh on every request (one joined query in `OrgContext`), so role
//! changes and removals take effect instantly on every pod without any
//! cross-pod invalidation.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::models::_entities::users;

/// Default freshness bound of a cached row.
pub const DEFAULT_TTL: Duration = Duration::from_secs(5);
/// Hard upper bound, whatever the configuration says.
const MAX_TTL: Duration = Duration::from_secs(10);
/// Size bound (one entry per active user on this pod).
const MAX_ENTRIES: usize = 10_000;

struct Entry {
    user: users::Model,
    email: String,
    display_name: String,
    at: Instant,
}

static CACHE: LazyLock<Mutex<HashMap<String, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, Entry>> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// TTL from `settings.auth.user_cache_ttl_ms` (`0` disables the cache —
/// the test config does, since suites mutate `users` rows directly),
/// capped at 10 s.
pub fn ttl(config: &loco_rs::config::Config) -> Duration {
    config
        .settings
        .as_ref()
        .and_then(|s| s.get("auth"))
        .and_then(|a| a.get("user_cache_ttl_ms"))
        .and_then(serde_json::Value::as_u64)
        .map_or(DEFAULT_TTL, Duration::from_millis)
        .min(MAX_TTL)
}

/// Cached row of `sub` when younger than `ttl` and still matching the
/// token profile.
pub fn get(sub: &str, email: &str, display_name: &str, ttl: Duration) -> Option<users::Model> {
    if ttl.is_zero() {
        return None;
    }
    let cache = lock();
    let e = cache.get(sub)?;
    (e.at.elapsed() < ttl && e.email == email && e.display_name == display_name)
        .then(|| e.user.clone())
}

/// Stores the row resolved for `sub` with the token profile it was
/// synced against.
pub fn put(sub: &str, email: &str, display_name: &str, user: users::Model) {
    let mut cache = lock();
    if cache.len() >= MAX_ENTRIES {
        cache.retain(|_, e| e.at.elapsed() < MAX_TTL);
        if cache.len() >= MAX_ENTRIES {
            cache.clear();
        }
    }
    cache.insert(
        sub.to_string(),
        Entry {
            user,
            email: email.to_string(),
            display_name: display_name.to_string(),
            at: Instant::now(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: i64) -> users::Model {
        let now = chrono::Utc::now().fixed_offset();
        users::Model {
            created_at: now,
            updated_at: now,
            id,
            idp_sub: Some("sub-test".into()),
            email: "a@example.com".into(),
            full_name: Some("A".into()),
            platform_admin: false,
        }
    }

    #[test]
    fn hit_requires_fresh_entry_and_unchanged_profile() {
        let ttl = Duration::from_millis(200);
        put("sub-cache-1", "a@example.com", "A", user(1));
        assert_eq!(
            get("sub-cache-1", "a@example.com", "A", ttl).map(|u| u.id),
            Some(1)
        );
        // IdP-side rename / email change: miss → sync path runs.
        assert!(get("sub-cache-1", "a@example.com", "B", ttl).is_none());
        assert!(get("sub-cache-1", "b@example.com", "A", ttl).is_none());
        // Disabled cache.
        assert!(get("sub-cache-1", "a@example.com", "A", Duration::ZERO).is_none());
        // Expired.
        std::thread::sleep(Duration::from_millis(250));
        assert!(get("sub-cache-1", "a@example.com", "A", ttl).is_none());
    }
}
