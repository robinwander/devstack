use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use tokio::sync::watch;

#[derive(Clone)]
pub(crate) struct HealthHandle {
    pub(crate) stop_flag: Arc<AtomicBool>,
    pub(crate) stats: Arc<Mutex<HealthSnapshot>>,
}

impl HealthHandle {
    pub(crate) fn new() -> Self {
        Self {
            stop_flag: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(Mutex::new(HealthSnapshot::default())),
        }
    }

    pub(crate) fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for HealthHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HealthHandle").finish_non_exhaustive()
    }
}

#[derive(Clone, Default, Debug)]
pub(crate) struct HealthSnapshot {
    pub(crate) passes: u64,
    pub(crate) failures: u64,
    pub(crate) consecutive_failures: u32,
    pub(crate) last_check_at: Option<String>,
    pub(crate) last_ok: Option<bool>,
}

#[derive(Clone, Debug)]
pub(crate) struct ServiceWatchHandle {
    pub(crate) stop_flag: Arc<AtomicBool>,
    pub(crate) paused: Arc<AtomicBool>,
}

impl ServiceWatchHandle {
    pub(crate) fn new(paused: bool) -> Self {
        Self {
            stop_flag: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(paused)),
        }
    }

    pub(crate) fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ApiCaptureHandle {
    pub(crate) stop_flag: Arc<AtomicBool>,
    drain_complete: watch::Receiver<bool>,
    drain_timeout: Duration,
}

impl ApiCaptureHandle {
    pub(crate) fn new(drain_complete: watch::Receiver<bool>, drain_timeout: Duration) -> Self {
        Self {
            stop_flag: Arc::new(AtomicBool::new(false)),
            drain_complete,
            drain_timeout,
        }
    }

    pub(crate) fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        self.spawn_drain_wait();
    }

    pub(crate) async fn stop_and_wait(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        self.wait_for_drain().await;
    }

    fn spawn_drain_wait(&self) {
        if *self.drain_complete.borrow() {
            return;
        }
        let mut drain_complete = self.drain_complete.clone();
        let drain_timeout = self.drain_timeout;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = tokio::time::timeout(drain_timeout, async move {
                    while !*drain_complete.borrow() {
                        if drain_complete.changed().await.is_err() {
                            break;
                        }
                    }
                })
                .await;
            });
        }
    }

    async fn wait_for_drain(&self) {
        if *self.drain_complete.borrow() {
            return;
        }
        let mut drain_complete = self.drain_complete.clone();
        let drain_timeout = self.drain_timeout;
        let _ = tokio::time::timeout(drain_timeout, async move {
            while !*drain_complete.borrow() {
                if drain_complete.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ServiceHandles {
    pub(crate) health: Option<HealthHandle>,
    pub(crate) watch: Option<ServiceWatchHandle>,
    pub(crate) api_capture: Option<ApiCaptureHandle>,
}
