//! Core types and trait definitions for the Kith contact store.
//!
//! This crate is deliberately free of HTTP and database dependencies.
//! All other crates depend on it; it depends on nothing proprietary.

pub mod error;
pub mod fact;
pub mod lifecycle;
pub mod store;
pub mod subject;

pub use error::{Error, Result};
