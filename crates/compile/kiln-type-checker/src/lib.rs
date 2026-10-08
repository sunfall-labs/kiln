//! Source type checking over Kiln-owned syntax snapshots.
//!
//! Source types describe TypeScript's contract, not a proof of JavaScript runtime values.
//! Optimizers must retain their guards or independently establish a Value Graph proof.
#![feature(allocator_api)]
mod engine;
mod libraries;
mod lower;
mod lower_modules;
mod lower_types;
mod lower_values;
mod plan;
mod snapshot;
mod support;
pub use libraries::STANDARD_LIBRARIES;
pub use snapshot::{
    CheckOptions, CheckResult, FactSelection, Libraries, SourceDiagnostic, SourceSnapshot,
    SourceType, TypeClass, TypeRole, check, project_roots,
};
/// Cache identity for the checker algorithms and owned-syntax adapter.
pub const CHECKER_VERSION: &str = "bun-43a42a836b/kiln-owned-v4";

#[cfg(test)]
mod tests;
