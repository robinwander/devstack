use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::systemd::SystemdManager;

#[derive(Clone, Debug)]
pub enum ReadinessKind {
    Tcp,
    Http {
        path: String,
        expect_min: u16,
        expect_max: u16,
    },
    LogRegex {
        pattern: String,
    },
    Cmd {
        command: String,
    },
    Delay {
        duration: Duration,
    },
    Exit,
    None,
}

#[derive(Clone, Debug)]
pub struct ReadinessSpec {
    pub kind: ReadinessKind,
    pub timeout: Duration,
}

impl ReadinessSpec {
    pub fn new(kind: ReadinessKind) -> Self {
        Self {
            kind,
            timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Clone)]
pub struct ReadinessContext {
    pub port: Option<u16>,
    pub scheme: String,
    pub log_path: PathBuf,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub unit_name: Option<String>,
    pub systemd: Option<Arc<dyn SystemdManager>>,
}