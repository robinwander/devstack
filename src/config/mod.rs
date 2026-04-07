pub mod env;
pub mod load;
pub mod model;
pub mod plan;
mod tests;
pub mod validate;

// Re-export the main types that were public in the original config.rs
pub use model::{
    ConfigFile, PortConfig, ReadinessConfig, ReadinessExit, ReadinessHttp, ReadinessTcp,
    ServiceConfig, StackConfig, StackPlan, TaskConfig, TaskDefinition, UniqueMap,
};

// Re-export public functions
pub use env::{resolve_env_map, resolve_env_vars};
pub use plan::topo_sort;
