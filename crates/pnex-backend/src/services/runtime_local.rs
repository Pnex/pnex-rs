//! Per-tokio-runtime singletons for Valkey connections and the background
//! tasks they feed (buses, writers).
//!
//! A `ConnectionManager` or a channel drained by a spawned task belongs to
//! the runtime that created it and dies with it. A server process has one
//! runtime, so these behave like plain process statics there; test binaries
//! boot one runtime per test, and each must get its own instance instead of
//! the dead one of the first test.

use std::future::Future;
use std::sync::Mutex;

use tokio::runtime::{Handle, Id};

/// Entries of finished runtimes (tests) are evicted oldest first.
const MAX_RUNTIMES: usize = 8;

pub struct PerRuntime<T> {
    slots: Mutex<Vec<(Id, T)>>,
}

impl<T: Clone> PerRuntime<T> {
    pub const fn new() -> Self {
        Self {
            slots: Mutex::new(Vec::new()),
        }
    }

    fn current() -> Option<Id> {
        Handle::try_current().ok().map(|h| h.id())
    }

    /// Value of the current runtime, if set.
    pub fn get(&self) -> Option<T> {
        let rt = Self::current()?;
        self.slots
            .lock()
            .expect("per-runtime slots")
            .iter()
            .find(|(id, _)| *id == rt)
            .map(|(_, v)| v.clone())
    }

    /// Sets the value of the current runtime; `false` when already set
    /// (the existing value is kept).
    pub fn set(&self, value: T) -> bool {
        let Some(rt) = Self::current() else {
            return false;
        };
        let mut slots = self.slots.lock().expect("per-runtime slots");
        if slots.iter().any(|(id, _)| *id == rt) {
            return false;
        }
        if slots.len() >= MAX_RUNTIMES {
            slots.remove(0);
        }
        slots.push((rt, value));
        true
    }

    /// Value of the current runtime, initialized by `init` on first use. A
    /// concurrent first use may run `init` twice; the first stored wins.
    pub async fn get_or_init<F, Fut>(&self, init: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        if let Some(v) = self.get() {
            return v;
        }
        let v = init().await;
        self.set(v.clone());
        self.get().unwrap_or(v)
    }
}

impl<T: Clone> Default for PerRuntime<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static SLOT: PerRuntime<u32> = PerRuntime::new();

    /// Each runtime sees its own value; the first set wins within one.
    #[test]
    fn values_are_isolated_per_runtime() {
        let rt1 = tokio::runtime::Runtime::new().expect("rt1");
        let rt2 = tokio::runtime::Runtime::new().expect("rt2");
        rt1.block_on(async {
            assert!(SLOT.set(1));
            assert!(!SLOT.set(9));
            assert_eq!(SLOT.get(), Some(1));
        });
        rt2.block_on(async {
            assert_eq!(SLOT.get(), None);
            assert_eq!(SLOT.get_or_init(|| async { 2 }).await, 2);
        });
        rt1.block_on(async { assert_eq!(SLOT.get(), Some(1)) });
        assert_eq!(SLOT.get(), None, "outside any runtime");
    }
}
