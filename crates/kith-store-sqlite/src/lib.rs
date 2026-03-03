//! SQLite backend for the Kith contact store.
//!
//! Uses [`sqlx`] with a `SqlitePool` for async database access.
//! Schema is managed by `sqlx` migrations in `migrations/`.

mod encode;
mod store;

pub mod error;

pub use error::{Error, Result};
pub use store::SqliteStore;

#[cfg(test)]
mod tests;
