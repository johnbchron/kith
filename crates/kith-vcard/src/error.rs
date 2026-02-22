//! Error types for the kith-vcard codec.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
  #[error("vCard missing BEGIN/END:VCARD envelope")]
  MissingEnvelope,

  #[error("malformed content-line: {0}")]
  MalformedContentLine(String),

  #[error("malformed parameter: {0}")]
  MalformedParam(String),

  #[error("N property has wrong number of components (expected 5, got {0})")]
  MalformedN(usize),

  #[error("ADR property has wrong number of components (expected 7, got {0})")]
  MalformedAdr(usize),

  #[error("invalid date in {property}: {value}")]
  InvalidDate { property: String, value: String },

  #[error("invalid IMPP URI: {0}")]
  InvalidImppUri(String),

  #[error("unsupported vCard version: {0}")]
  UnsupportedVersion(String),

  #[error("invalid photo path: {0}")]
  InvalidPhotoPath(String),

  #[error("JSON error: {0}")]
  Json(#[from] serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
