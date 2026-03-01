//! Error types for the kith-vcard codec.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
  #[error("vCard missing BEGIN/END:VCARD envelope")]
  MissingEnvelope,

  #[error("malformed content-line: {0}")]
  MalformedContentLine(String),

  #[error("invalid date in {property}: {value}")]
  InvalidDate { property: String, value: String },

  #[error("invalid IMPP URI: {0}")]
  InvalidImppUri(String),

  #[error("JSON error: {0}")]
  Json(#[from] serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
