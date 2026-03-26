pub mod model;
pub mod coordinator;
pub mod probes;
pub mod port_ownership;

// Re-export the main types that were public in the original readiness.rs
pub use model::{ReadinessKind, ReadinessSpec, ReadinessContext};
pub use coordinator::{wait_for_ready, check_ready_once};
pub use probes::{readiness_url, is_success_status};
pub use port_ownership::{verify_port_binding, port_binding_info, PortBindingInfo};

#[cfg(target_os = "linux")]
pub use port_ownership::linux_port_binding_info;