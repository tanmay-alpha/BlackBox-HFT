//! BlackBox HFT application layer.
//!
//! Wires the dataset store, the strategy library, the simulation adapter and the forensics rules
//! into one pipeline, persists completed runs, and exposes everything over the HTTP API described
//! in `docs/API.md` and through the `blackbox` command line.
//!
//! Nothing in this crate talks to an exchange or makes an outbound network request. Every number
//! it returns comes from a simulation over historical or synthetic data.

pub mod api;
pub mod datasets;
pub mod error;
pub mod pipeline;
pub mod presets;
pub mod server;
pub mod store;

pub use error::AppError;

/// Shown by the health endpoint and printed by every command that reports results.
pub const DISCLAIMER: &str = "Research only. Every order, fill, queue position and P&L figure is \
     the output of a simulation over historical or synthetic market data under stated latency, \
     queue and fee assumptions. Nothing here is a real exchange execution.";
