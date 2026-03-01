//! HTTP Basic-auth extractor and standalone verifier.

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{
  extract::FromRequestParts,
  http::{HeaderMap, request::Parts},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use kith_core::store::ContactStore;

use crate::{AppState, error::Error};

/// Credentials accepted as valid for this server instance.
#[derive(Clone)]
pub struct AuthConfig {
  pub username:      String,
  /// PHC string produced by argon2, e.g. `$argon2id$v=19$…`
  pub password_hash: String,
}

/// Zero-size marker: present in the handler means the request was
/// authenticated.
pub struct Authenticated;

/// Verify credentials directly from headers — used by manual dispatch handlers.
pub fn verify_auth(
  headers: &HeaderMap,
  config: &AuthConfig,
) -> Result<(), Error> {
  let header_val = headers
    .get(axum::http::header::AUTHORIZATION)
    .and_then(|v| v.to_str().ok())
    .ok_or(Error::Unauthorized)?;

  let encoded = header_val
    .strip_prefix("Basic ")
    .ok_or(Error::Unauthorized)?;

  let decoded = B64.decode(encoded).map_err(|_| Error::Unauthorized)?;
  let creds = std::str::from_utf8(&decoded).map_err(|_| Error::Unauthorized)?;

  let (username, password) =
    creds.split_once(':').ok_or(Error::Unauthorized)?;

  if username != config.username {
    return Err(Error::Unauthorized);
  }

  let parsed_hash = PasswordHash::new(&config.password_hash)
    .map_err(|_| Error::Unauthorized)?;

  Argon2::default()
    .verify_password(password.as_bytes(), &parsed_hash)
    .map_err(|_| Error::Unauthorized)?;

  Ok(())
}

impl<S> FromRequestParts<AppState<S>> for Authenticated
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  type Rejection = Error;

  async fn from_request_parts(
    parts: &mut Parts,
    state: &AppState<S>,
  ) -> Result<Self, Self::Rejection> {
    verify_auth(&parts.headers, &state.auth)?;
    Ok(Authenticated)
  }
}

#[cfg(test)]
mod tests {
  use axum::http::{Request, header};
  use kith_store_sqlite::SqliteStore;

  use super::*;
  use crate::{AppState, test_helpers::make_state};

  async fn extract(
    req: Request<axum::body::Body>,
    state: &AppState<SqliteStore>,
  ) -> Result<Authenticated, Error> {
    let (mut parts, _) = req.into_parts();
    Authenticated::from_request_parts(&mut parts, state).await
  }

  fn basic(user: &str, pass: &str) -> String {
    let encoded = B64.encode(format!("{user}:{pass}"));
    format!("Basic {encoded}")
  }

  #[tokio::test]
  async fn correct_credentials() {
    let state = make_state("secret").await;
    let req = Request::builder()
      .header(header::AUTHORIZATION, basic("user", "secret"))
      .body(axum::body::Body::empty())
      .unwrap();
    assert!(extract(req, &state).await.is_ok());
  }

  #[tokio::test]
  async fn wrong_password() {
    let state = make_state("secret").await;
    let req = Request::builder()
      .header(header::AUTHORIZATION, basic("user", "wrong"))
      .body(axum::body::Body::empty())
      .unwrap();
    assert!(matches!(
      extract(req, &state).await,
      Err(Error::Unauthorized)
    ));
  }

  #[tokio::test]
  async fn missing_header() {
    let state = make_state("secret").await;
    let req = Request::builder().body(axum::body::Body::empty()).unwrap();
    assert!(matches!(
      extract(req, &state).await,
      Err(Error::Unauthorized)
    ));
  }

  #[tokio::test]
  async fn invalid_base64() {
    let state = make_state("secret").await;
    let req = Request::builder()
      .header(header::AUTHORIZATION, "Basic !!!not-base64!!!")
      .body(axum::body::Body::empty())
      .unwrap();
    assert!(matches!(
      extract(req, &state).await,
      Err(Error::Unauthorized)
    ));
  }
}
