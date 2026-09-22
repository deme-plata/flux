//! Datacenter control node: real local work and mesh, not a physical facility.
pub mod config;
pub mod runtime;
pub mod storage;
pub mod planning;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
