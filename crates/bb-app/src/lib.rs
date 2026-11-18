//! BlackBox HFT application layer.
//!
//! Composition root (TARGET_ARCHITECTURE §3): the `/api/v1` HTTP API ([`v1`]), the leased job
//! worker ([`worker`]), admission ([`admission`]), integrity-checked artefact reads
//! ([`artifacts`]), the dataset registry ([`registry`]), configuration from the environment
//! ([`config`], the only place that reads it) and the `blackbox` command line. The offline
//! pipeline ([`pipeline`]) and the legacy file-store tooling ([`store`], [`server`]) keep the
//! local, DB-free commands working.
//!
//! Nothing in this crate talks to an exchange. Every number it returns comes from a simulation
//! over historical or synthetic data.

pub mod admission;
pub mod analytics;
pub mod api;
pub mod artifacts;
pub mod canonical;
pub mod config;
pub mod datasets;
pub mod demos;
pub mod error;
pub mod metrics;
pub mod pipeline;
pub mod presets;
pub mod queue_study;
pub mod registry;
mod report;
pub mod runtime;
pub mod seed;
/// Deprecated legacy `/api` router (unauthenticated, file store). Served only by `blackbox
/// demo` and, in the local profile, as the fallback of `blackbox serve` until the web UI uses
/// `/api/v1` (TARGET_ARCHITECTURE §11); never deployed to the hosted profile.
pub mod server;
pub mod store;
pub mod study;
pub mod v1;
pub mod wire;
pub mod worker;

pub use error::AppError;

/// Shown by the health endpoint and printed by every command that reports results.
pub const DISCLAIMER: &str = "Research only. Every order, fill, queue position and P&L figure is \
     the output of a simulation over historical or synthetic market data under stated latency, \
     queue and fee assumptions. Simulated orders never move replayed prices or consume displayed \
     depth (no market impact). Nothing here is a real exchange execution.";
