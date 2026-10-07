//! Wyhash helpers used by the imported checker.
mod wyhash;
pub use wyhash::{hash, hash_ascii_lowercase, hash_with_seed, hash32};
