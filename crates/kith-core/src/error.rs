//! Error types for `kith-core`.

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum Error {
  #[error("subject not found: {0}")]
  SubjectNotFound(Uuid),

  #[error("fact not found: {0}")]
  FactNotFound(Uuid),

  #[error("fact {0} is already superseded")]
  AlreadySuperseded(Uuid),

  #[error("fact {0} is already retracted")]
  AlreadyRetracted(Uuid),

  #[error("cannot supersede a fact with itself")]
  SelfSupersession,

  #[error("unknown fact type discriminant: {0:?}")]
  UnknownFactType(String),

  #[error("serialization error: {0}")]
  Serialization(#[from] serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
