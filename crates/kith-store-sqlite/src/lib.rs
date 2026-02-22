//! SQLite backend for the Kith contact store.
//!
//! Wraps [`tokio_rusqlite`] so all database access runs on a dedicated thread
//! pool without blocking the async runtime.

mod encode;
mod schema;
mod store;

pub mod error;

pub use error::{Error, Result};
pub use store::SqliteStore;

#[cfg(test)]
mod tests;
