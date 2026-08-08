//! fluxc-analytics — the measurement layer of the fluxc stack (v0.41 split).
//! Sits on fluxc-util only; editing this crate never rebuilds fluxc-core.
pub mod benchmark;
pub mod heatmap;
pub mod predict;
pub mod qspec;
pub mod tune;
pub mod xray;
