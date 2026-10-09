//! Local validation of Rauthy JWTs via JWKS (RS256), no introspection —
//! continuity with the legacy implementation, **with the hardenings** decided
//! in Phase 3 (Phase 0 report §3.4-3.5): explicit `iss` check, audience
//! restricted to `{client_id, "account"}`, RS256 algorithm only.
//!
//! JWKS are cached in memory and refreshed when an unknown `kid` shows up
//! (Rauthy key rotation) — the legacy implementation cached for 1 h without
//! refresh on unknown `kid`.
//!
//! Refresh discipline (a JWKS fetch must never be per-request):
//! - **single-flight**: concurrent misses share one fetch (async mutex held
//!   only by the refresher, never on the verification hot path);
//! - **cooldown**: at most one miss-triggered fetch per [`REFRESH_COOLDOWN`];
//!   unknown kids seen meanwhile are negative-cached (bounded map) and
//!   rejected without any network call;
//! - **TTL**: known keys are re-fetched after [`KEYS_TTL`] (opportunistic,
//!   by the first request that notices, others keep using the cache);
//! - a failed fetch **never wipes** the current key set. The `pnex` client is configured on the Rauthy
//! side with `access_token_alg: RS256` (deploy/rauthy/bootstrap/clients.json).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::Mutex;

use super::claims::Claims;
use super::settings::RauthySettings;

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("token absent ou malformé")]
    Malformed,
    #[error("signature invalide ou algorithme non autorisé")]
    BadSignature,
    #[error("clé de signature inconnue (kid) après rafraîchissement JWKS")]
    UnknownKid,
    #[error("token expiré")]
    Expired,
    #[error("émetteur (iss) invalide")]
    BadIssuer,
    #[error("audience (aud) invalide")]
    BadAudience,
    #[error("Rauthy injoignable pour les JWKS : {0}")]
    JwksUnreachable(String),
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    /// Absents sur les clés non-RSA (Rauthy publie aussi des clés OKP/EdDSA
    /// dans le même ensemble) — les entrées sans `n`/`e` sont ignorées.
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
}

/// Minimum delay between two miss-triggered JWKS fetches.
pub const REFRESH_COOLDOWN: Duration = Duration::from_secs(30);
/// Age after which the known key set is refreshed proactively.
pub const KEYS_TTL: Duration = Duration::from_secs(3600);
/// Retry delay while no key was ever loaded (bounded by the cooldown).
const EMPTY_SET_RETRY: Duration = Duration::from_secs(2);
/// Upper bound of the unknown-kid negative cache (attacker-controlled input).
const NEGATIVE_CACHE_MAX: usize = 1024;

#[derive(Default)]
struct KeyState {
    keys: HashMap<String, Arc<DecodingKey>>,
    /// Last successful fetch.
    fetched_at: Option<Instant>,
    /// Last fetch attempt (success or failure) — drives the cooldown.
    attempted_at: Option<Instant>,
    /// Unknown kids → when they were confirmed unknown.
    unknown: HashMap<String, Instant>,
}

pub struct JwksVerifier {
    issuer: String,
    /// Audience acceptée : le client PNEX + "account" (héritage Keycloak,
    /// conservé par prudence). Un token émis pour un autre client est rejeté.
    audiences: Vec<String>,
    jwks_url: String,
    http: reqwest::Client,
    /// Hot path: short, non-async critical sections only.
    state: RwLock<KeyState>,
    /// Single-flight guard: only the task performing a fetch holds it.
    refresh_lock: Mutex<()>,
    cooldown: Duration,
    ttl: Duration,
    /// Number of JWKS fetches performed (observability + tests).
    fetches: std::sync::atomic::AtomicU64,
}

impl JwksVerifier {
    pub fn new(settings: &RauthySettings) -> Self {
        Self::with_timings(settings, REFRESH_COOLDOWN, KEYS_TTL)
    }

    /// Same as [`Self::new`] with explicit cooldown/TTL (tests).
    pub fn with_timings(settings: &RauthySettings, cooldown: Duration, ttl: Duration) -> Self {
        Self {
            issuer: settings.issuer(),
            audiences: vec![settings.client_id.clone(), "account".into()],
            jwks_url: settings.jwks_url(),
            http: reqwest::Client::builder()
                // Rauthy refuse les requêtes sans User-Agent (400).
                .user_agent("pnex-server")
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("reqwest client"),
            state: RwLock::new(KeyState::default()),
            refresh_lock: Mutex::new(()),
            cooldown,
            ttl,
            fetches: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Valide un access token : signature RS256 (JWKS), `iss`, `aud`, `exp`.
    pub async fn verify(&self, token: &str) -> Result<Claims, VerifyError> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| VerifyError::Malformed)?;
        let kid = header.kid.ok_or(VerifyError::Malformed)?;
        let key = self.key_for(&kid).await?;

        let mut validation = Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&self.audiences);
        validation.validate_exp = true;

        let data = decode::<Claims>(token, &key, &validation).map_err(|err| match err.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => VerifyError::Expired,
            jsonwebtoken::errors::ErrorKind::InvalidIssuer => VerifyError::BadIssuer,
            jsonwebtoken::errors::ErrorKind::InvalidAudience => VerifyError::BadAudience,
            _ => VerifyError::BadSignature,
        })?;
        Ok(data.claims)
    }

    fn read_state(&self) -> std::sync::RwLockReadGuard<'_, KeyState> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write_state(&self) -> std::sync::RwLockWriteGuard<'_, KeyState> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Resolves the decoding key of `kid` under the refresh discipline
    /// described in the module docs.
    async fn key_for(&self, kid: &str) -> Result<Arc<DecodingKey>, VerifyError> {
        let now = Instant::now();
        let (cached, stale, negative) = {
            let st = self.read_state();
            let stale = st
                .fetched_at
                .is_none_or(|t| now.duration_since(t) >= self.ttl);
            let negative = st
                .unknown
                .get(kid)
                .is_some_and(|t| now.duration_since(*t) < self.cooldown);
            (st.keys.get(kid).cloned(), stale, negative)
        };

        if let Some(key) = cached {
            // Known kid: an expired key set is refreshed by whoever grabs the
            // lock first; everyone else keeps serving from the cache.
            if !stale {
                return Ok(key);
            }
            let Ok(_guard) = self.refresh_lock.try_lock() else {
                return Ok(key);
            };
            if self.cooldown_elapsed(Instant::now()) {
                match self.fetch().await {
                    // A kid dropped by the IdP (revoked key) stops verifying
                    // as soon as the fresh set is in.
                    Ok(()) => {
                        return self
                            .read_state()
                            .keys
                            .get(kid)
                            .cloned()
                            .ok_or(VerifyError::UnknownKid);
                    }
                    Err(err) => {
                        tracing::warn!(%err, "periodic JWKS refresh failed (keeping cached keys)");
                    }
                }
            }
            return Ok(key);
        }
        if negative {
            return Err(VerifyError::UnknownKid);
        }

        // Miss: single-flight. Whoever waited on the lock re-checks first —
        // the fetch it waited for may already have brought the key.
        let _guard = self.refresh_lock.lock().await;
        if let Some(key) = self.read_state().keys.get(kid).cloned() {
            return Ok(key);
        }
        if self.cooldown_elapsed(Instant::now()) {
            self.fetch().await?;
            if let Some(key) = self.read_state().keys.get(kid).cloned() {
                return Ok(key);
            }
        }
        if !self.read_state().keys.is_empty() {
            self.remember_unknown(kid);
        }
        Err(VerifyError::UnknownKid)
    }

    fn cooldown_elapsed(&self, now: Instant) -> bool {
        let st = self.read_state();
        // No usable key yet (IdP down at boot): retry sooner so a short
        // outage does not lock everyone out for a whole cooldown.
        let cooldown = if st.keys.is_empty() {
            self.cooldown.min(EMPTY_SET_RETRY)
        } else {
            self.cooldown
        };
        st.attempted_at
            .is_none_or(|t| now.duration_since(t) >= cooldown)
    }

    fn remember_unknown(&self, kid: &str) {
        let now = Instant::now();
        let cooldown = self.cooldown;
        let mut st = self.write_state();
        if st.unknown.len() >= NEGATIVE_CACHE_MAX {
            st.unknown.retain(|_, t| now.duration_since(*t) < cooldown);
            if st.unknown.len() >= NEGATIVE_CACHE_MAX {
                st.unknown.clear();
            }
        }
        st.unknown.insert(kid.to_string(), now);
    }

    /// Fetches the JWKS; must be called with `refresh_lock` held. On failure
    /// the current key set is left untouched.
    async fn fetch(&self) -> Result<(), VerifyError> {
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.write_state().attempted_at = Some(Instant::now());
        let jwks: Jwks = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .and_then(|resp| resp.error_for_status())
            .map_err(|err| VerifyError::JwksUnreachable(err.to_string()))?
            .json()
            .await
            .map_err(|err| VerifyError::JwksUnreachable(err.to_string()))?;

        let mut keys = HashMap::new();
        for jwk in jwks.keys {
            if jwk.kty != "RSA" {
                continue;
            }
            if let Some(alg) = &jwk.alg {
                if alg != "RS256" {
                    continue;
                }
            }
            // Clés non-RSA (OKP/EdDSA) : pas de composants n/e → ignorées.
            let (Some(n), Some(e)) = (jwk.n, jwk.e) else {
                continue;
            };
            if let Ok(key) = DecodingKey::from_rsa_components(&n, &e) {
                keys.insert(jwk.kid, Arc::new(key));
            }
        }
        if keys.is_empty() {
            // An empty/unusable set is treated as a failed fetch: never
            // replace working keys with nothing.
            return Err(VerifyError::JwksUnreachable(
                "JWKS contains no usable RS256 key".into(),
            ));
        }
        let mut st = self.write_state();
        st.unknown.retain(|kid, _| !keys.contains_key(kid));
        st.keys = keys;
        st.fetched_at = Some(Instant::now());
        Ok(())
    }
}

/// Registre process-global : un vérifieur par issuer (dev/test/prod peuvent
/// pointer vers des IdP différents dans le même process de test). Read path
/// is a shared lock, the write lock is only taken to insert a new issuer.
static VERIFIERS: std::sync::OnceLock<RwLock<HashMap<String, Arc<JwksVerifier>>>> =
    std::sync::OnceLock::new();

pub async fn verifier_for(settings: &RauthySettings) -> Arc<JwksVerifier> {
    let registry = VERIFIERS.get_or_init(|| RwLock::new(HashMap::new()));
    let issuer = settings.issuer();
    if let Some(v) = registry
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(&issuer)
    {
        return v.clone();
    }
    registry
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .entry(issuer)
        .or_insert_with(|| Arc::new(JwksVerifier::new(settings)))
        .clone()
}
