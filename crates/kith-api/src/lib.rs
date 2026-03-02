//! JSON REST API for Kith.
//!
//! Exposes an axum [`Router`] backed by any [`kith_core::store::ContactStore`].
//! Auth, TLS, and transport concerns are the caller's responsibility.
//!
//! # Mounting
//!
//! ```rust,ignore
//! .nest("/api", kith_api::api_router(store.clone()))
//! ```

pub mod error;
pub mod facts;
pub mod search;
pub mod subjects;

use std::sync::Arc;

use axum::{
  Router,
  routing::{get, post},
};
use kith_core::store::ContactStore;

pub use error::ApiError;

/// Build a fully-materialised API router for `store`.
///
/// The returned `Router<()>` can be nested into any parent router regardless
/// of its own state type.
pub fn api_router<S>(store: Arc<S>) -> Router<()>
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  Router::new()
    // Subjects
    .route("/subjects", get(subjects::list::<S>).post(subjects::create::<S>))
    .route("/subjects/{id}", get(subjects::get_one::<S>))
    // Facts
    .route("/facts", get(facts::list::<S>).post(facts::create::<S>))
    .route("/facts/{id}", get(facts::get_one::<S>))
    .route("/facts/{id}/supersede", post(facts::supersede_one::<S>))
    .route("/facts/{id}/retract", post(facts::retract_one::<S>))
    // Search
    .route("/search", get(search::handler::<S>))
    .with_state(store)
}
