use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

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

#[derive(Clone, Debug, Default)]
pub(crate) struct ServiceHandles {
    pub(crate) health: Option<HealthHandle>,
    pub(crate) watch: Option<ServiceWatchHandle>,
}
