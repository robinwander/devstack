pub mod model;
pub mod load;
pub mod validate;
pub mod plan;
pub mod env;
mod tests;

// Re-export the main types that were public in the original config.rs
pub use model::{
    ConfigFile, StackConfig, ServiceConfig, TaskConfig, TaskDefinition, 
    PortConfig, ReadinessConfig, ReadinessTcp, ReadinessHttp, ReadinessExit, 
    StackPlan, UniqueMap
};

// Re-export public functions
pub use plan::topo_sort;
pub use env::{resolve_env_vars, resolve_env_map};