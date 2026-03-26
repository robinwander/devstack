use tokio::sync::Mutex;
use crate::api::NavigationIntent;

/// Store for managing navigation intent state
pub struct NavigationStore {
    inner: Mutex<Option<NavigationIntent>>,
}

impl NavigationStore {
    /// Create a new navigation store
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    /// Set the navigation intent
    pub async fn set_intent(&self, intent: NavigationIntent) {
        let mut guard = self.inner.lock().await;
        *guard = Some(intent);
    }

    /// Get the current navigation intent
    pub async fn get_intent(&self) -> Option<NavigationIntent> {
        let guard = self.inner.lock().await;
        guard.clone()
    }

    /// Clear the navigation intent
    pub async fn clear_intent(&self) -> bool {
        let mut guard = self.inner.lock().await;
        let had_intent = guard.is_some();
        *guard = None;
        had_intent
    }

    /// Update the navigation intent if one exists
    pub async fn update_intent<F>(&self, updater: F) -> bool
    where
        F: FnOnce(&mut NavigationIntent),
    {
        let mut guard = self.inner.lock().await;
        if let Some(ref mut intent) = *guard {
            updater(intent);
            true
        } else {
            false
        }
    }
}

impl Default for NavigationStore {
    fn default() -> Self {
        Self::new()
    }
}