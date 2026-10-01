//! Politique de livraison at-least-once (D53) : 3 retries à backoff fixe
//! 1 s / 5 s / 25 s, timeout 10 s par tentative (le timeout est posé sur
//! le client reqwest partagé, cf. `channels`).
//!
//! Seuls les échecs [`NotifyError::retryable`] déclenchent un retry — un
//! 4xx ou une config invalide reproduirait la même erreur.

use std::time::Duration;

use crate::error::NotifyError;

/// Backoff par défaut (D53).
pub const DEFAULT_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(25),
];

/// Exécute `op` avec retry at-least-once : 1 tentative initiale + 3
/// retries sur échec transitoire.
pub async fn with_retry<T, F, Fut>(op: F) -> Result<T, NotifyError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, NotifyError>>,
{
    with_retry_delays(DEFAULT_DELAYS, op).await
}

/// Variante à délais injectables (tests) — même sémantique.
pub async fn with_retry_delays<T, F, Fut>(
    delays: [Duration; 3],
    mut op: F,
) -> Result<T, NotifyError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, NotifyError>>,
{
    let mut last = None;
    for attempt in 0..=delays.len() {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let retryable = e.retryable();
                last = Some(e);
                if !retryable || attempt == delays.len() {
                    break;
                }
                tokio::time::sleep(delays[attempt]).await;
            }
        }
    }
    Err(last.expect("au moins une tentative"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn tiny() -> [Duration; 3] {
        [Duration::from_millis(1); 3]
    }

    #[tokio::test]
    async fn reussite_premiere_tentative() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let r = with_retry_delays(tiny(), move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok::<_, NotifyError>(42)
            }
        })
        .await;
        assert_eq!(r.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_sur_transitoire_puis_succes() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let r: Result<&str, _> = with_retry_delays(tiny(), move || {
            let c = c.clone();
            async move {
                if c.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err(NotifyError::Network("connection refused".into()))
                } else {
                    Ok("ok")
                }
            }
        })
        .await;
        assert_eq!(r.unwrap(), "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn pas_de_retry_sur_4xx() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let r: Result<(), _> = with_retry_delays(tiny(), move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(NotifyError::Http { status: 400 })
            }
        })
        .await;
        assert!(matches!(r.unwrap_err(), NotifyError::Http { status: 400 }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn epuise_les_trois_retries() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let r: Result<(), _> = with_retry_delays(tiny(), move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(NotifyError::Http { status: 503 })
            }
        })
        .await;
        assert!(matches!(r.unwrap_err(), NotifyError::Http { status: 503 }));
        // 1 tentative + 3 retries.
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }
}
