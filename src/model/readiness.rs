use std::time::Duration;

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
