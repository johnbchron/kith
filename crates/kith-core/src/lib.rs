//! Core types and trait definitions for the Kith contact store.
//!
//! This crate is deliberately free of HTTP and database dependencies.
//! All other crates depend on it; it depends on nothing proprietary.

// We intentionally use native `async fn` in traits (stabilised in Rust 1.75).
// Suppress the advisory lint about `Send` bounds on the returned futures.
#![allow(async_fn_in_trait)]

pub mod error;
pub mod fact;
pub mod lifecycle;
pub mod store;
pub mod subject;

pub use error::{Error, Result};
