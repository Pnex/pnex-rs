//! Browser cache of media bytes (web only, Cache Storage).
//!
//! Splats and panoramas weigh tens of MB and rarely change: each download
//! is kept under its API path with the server `ETag` (the version id, an
//! immutable blob). A later view still asks the server — with
//! `If-None-Match`, so the org scoping and the current version are always
//! re-checked — and a 304 serves the local copy without a byte of body.
//! Cache Storage rather than the HTTP cache: browsers refuse HTTP cache
//! entries above a few tens of MB, exactly the splats.
//!
//! Bounded by [`BUDGET_BYTES`] (oldest entries evicted first) and purged at
//! logout (no media left behind on a shared computer). Every failure
//! (insecure context, quota, private mode) degrades to "no cache".

#[cfg(target_arch = "wasm32")]
mod imp {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    const CACHE_NAME: &str = "pnex-media-v1";
    const SIZE_HEADER: &str = "x-pnex-size";
    /// Total size kept before evicting the oldest entries.
    const BUDGET_BYTES: f64 = 1_500_000_000.0;

    /// A cached copy and the validator it was stored with.
    pub struct Cached {
        response: web_sys::Response,
        pub etag: String,
    }

    impl Cached {
        /// `blob:` URL of the cached bytes (no copy through wasm memory).
        pub async fn blob_url(self) -> Option<String> {
            let blob = JsFuture::from(self.response.blob().ok()?).await.ok()?;
            let blob: web_sys::Blob = blob.dyn_into().ok()?;
            web_sys::Url::create_object_url_with_blob(&blob).ok()
        }
    }

    async fn open() -> Option<web_sys::Cache> {
        let caches = web_sys::window()?.caches().ok()?;
        let cache = JsFuture::from(caches.open(CACHE_NAME)).await.ok()?;
        cache.dyn_into().ok()
    }

    /// Cached copy of `key` (an API path), if any.
    pub async fn lookup(key: &str) -> Option<Cached> {
        let cache = open().await?;
        let found = JsFuture::from(cache.match_with_str(key)).await.ok()?;
        let response: web_sys::Response = found.dyn_into().ok()?;
        let etag = response.headers().get("etag").ok()??;
        Some(Cached { response, etag })
    }

    /// Stores `blob` under `key` with its validator, then trims the cache.
    pub async fn store(key: &str, blob: &web_sys::Blob, etag: &str) {
        let Some(cache) = open().await else {
            return;
        };
        let Ok(headers) = web_sys::Headers::new() else {
            return;
        };
        let _ = headers.set("etag", etag);
        let _ = headers.set(SIZE_HEADER, &blob.size().to_string());
        let init = web_sys::ResponseInit::new();
        init.set_headers_headers(&headers);
        let Ok(response) = web_sys::Response::new_with_opt_blob_and_init(Some(blob), &init) else {
            return;
        };
        if JsFuture::from(cache.put_with_str(key, &response))
            .await
            .is_err()
        {
            // Quota exceeded or storage unavailable: just not cached.
            return;
        }
        evict(&cache).await;
    }

    /// Drops the oldest entries (insertion order) beyond the budget.
    async fn evict(cache: &web_sys::Cache) {
        let Ok(keys) = JsFuture::from(cache.keys()).await else {
            return;
        };
        let keys: js_sys::Array = keys.unchecked_into();
        let mut entries = Vec::new();
        let mut total = 0.0;
        for key in keys.iter() {
            let request: web_sys::Request = key.unchecked_into();
            let size = match JsFuture::from(cache.match_with_request(&request)).await {
                Ok(found) => found
                    .dyn_into::<web_sys::Response>()
                    .ok()
                    .and_then(|r| r.headers().get(SIZE_HEADER).ok().flatten())
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0),
                Err(_) => 0.0,
            };
            total += size;
            entries.push((request, size));
        }
        // Never evict the newest entry (the one just stored).
        let evictable = entries.len().saturating_sub(1);
        for (request, size) in entries.into_iter().take(evictable) {
            if total <= BUDGET_BYTES {
                break;
            }
            let _ = JsFuture::from(cache.delete_with_request(&request)).await;
            total -= size;
        }
    }

    /// Deletes the whole media cache (logout).
    pub fn clear() {
        let Some(caches) = web_sys::window().and_then(|w| w.caches().ok()) else {
            return;
        };
        let promise = caches.delete(CACHE_NAME);
        wasm_bindgen_futures::spawn_local(async move {
            let _ = JsFuture::from(promise).await;
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use imp::{lookup, store};

/// Deletes the media cache (logout); no-op on native targets.
pub fn clear() {
    #[cfg(target_arch = "wasm32")]
    imp::clear();
}
