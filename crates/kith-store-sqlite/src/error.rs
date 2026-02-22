//! Error type for `kith-store-sqlite`.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
  #[error("core error: {0}")]
  Core(#[from] kith_core::Error),

  #[error("database error: {0}")]
  Database(#[from] tokio_rusqlite::Error),

  #[error("json error: {0}")]
  Json(#[from] serde_json::Error),

  #[error("uuid parse error: {0}")]
  Uuid(#[from] uuid::Error),

  #[error("date/time parse error: {0}")]
  DateParse(String),

  /// Attempted to supersede or retract a fact that was not found.
  #[error("fact not found: {0}")]
  FactNotFound(uuid::Uuid),

  #[error("fact {0} is already superseded")]
  AlreadySuperseded(uuid::Uuid),

  #[error("fact {0} is already retracted")]
  AlreadyRetracted(uuid::Uuid),

  #[error("subject not found: {0}")]
  SubjectNotFound(uuid::Uuid),

  #[error("cannot supersede a fact with itself")]
  SelfSupersession,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
