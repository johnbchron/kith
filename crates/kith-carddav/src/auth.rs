//! HTTP Basic-auth extractor and standalone verifier.

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::extract::FromRequestParts;
use axum::http::{HeaderMap, request::Parts};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

use crate::{AppState, error::Error};
use kith_core::store::ContactStore;

/// Credentials accepted as valid for this server instance.
#[derive(Clone)]
pub struct AuthConfig {
  pub username:      String,
  /// PHC string produced by argon2, e.g. `$argon2id$v=19$…`
  pub password_hash: String,
}

/// Zero-size marker: present in the handler means the request was authenticated.
pub struct Authenticated;

/// Verify credentials directly from headers — used by manual dispatch handlers.
pub fn verify_auth(headers: &HeaderMap, config: &AuthConfig) -> Result<(), Error> {
  let header_val = headers
    .get(axum::http::header::AUTHORIZATION)
    .and_then(|v| v.to_str().ok())
    .ok_or(Error::Unauthorized)?;

  let encoded = header_val
    .strip_prefix("Basic ")
    .ok_or(Error::Unauthorized)?;

  let decoded = B64.decode(encoded).map_err(|_| Error::Unauthorized)?;
  let creds   = std::str::from_utf8(&decoded).map_err(|_| Error::Unauthorized)?;

  let (username, password) = creds.split_once(':').ok_or(Error::Unauthorized)?;

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
  use super::*;
  use std::sync::Arc;
  use axum::http::{Request, header};
  use crate::{AppState, ServerConfig};
  use std::path::PathBuf;

  // A minimal no-op store for testing auth only.
  #[derive(Clone)]
  struct NoopStore;

  impl kith_core::store::ContactStore for NoopStore {
    type Error = std::convert::Infallible;
    async fn add_subject(&self, _: kith_core::subject::SubjectKind) -> Result<kith_core::subject::Subject, Self::Error> { unimplemented!() }
    async fn add_subject_with_id(&self, _: uuid::Uuid, _: kith_core::subject::SubjectKind) -> Result<kith_core::subject::Subject, Self::Error> { unimplemented!() }
    async fn get_subject(&self, _: uuid::Uuid) -> Result<Option<kith_core::subject::Subject>, Self::Error> { unimplemented!() }
    async fn list_subjects(&self, _: Option<kith_core::subject::SubjectKind>) -> Result<Vec<kith_core::subject::Subject>, Self::Error> { unimplemented!() }
    async fn record_fact(&self, _: kith_core::fact::NewFact) -> Result<kith_core::fact::Fact, Self::Error> { unimplemented!() }
    async fn supersede(&self, _: uuid::Uuid, _: kith_core::fact::NewFact) -> Result<(kith_core::lifecycle::Supersession, kith_core::fact::Fact), Self::Error> { unimplemented!() }
    async fn retract(&self, _: uuid::Uuid, _: Option<String>) -> Result<kith_core::lifecycle::Retraction, Self::Error> { unimplemented!() }
    async fn get_facts(&self, _: uuid::Uuid, _: Option<chrono::DateTime<chrono::Utc>>, _: bool) -> Result<Vec<kith_core::lifecycle::ResolvedFact>, Self::Error> { unimplemented!() }
    async fn materialize(&self, _: uuid::Uuid, _: Option<chrono::DateTime<chrono::Utc>>) -> Result<Option<kith_core::lifecycle::ContactView>, Self::Error> { unimplemented!() }
    async fn search(&self, _: &kith_core::store::FactQuery) -> Result<Vec<kith_core::subject::Subject>, Self::Error> { unimplemented!() }
  }

  fn make_state(password: &str) -> AppState<NoopStore> {
    use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
    use rand_core::OsRng;
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
      .hash_password(password.as_bytes(), &salt)
      .unwrap()
      .to_string();

    AppState {
      store:  Arc::new(NoopStore),
      config: Arc::new(ServerConfig {
        host:               "127.0.0.1".to_string(),
        port:               5232,
        base_url:           "http://localhost:5232".to_string(),
        addressbook:        "personal".to_string(),
        store_path:         PathBuf::from(":memory:"),
        auth_username:      "user".to_string(),
        auth_password_hash: hash.clone(),
      }),
      auth: Arc::new(AuthConfig {
        username:      "user".to_string(),
        password_hash: hash,
      }),
    }
  }

  async fn extract(req: Request<axum::body::Body>, state: &AppState<NoopStore>) -> Result<Authenticated, Error> {
    let (mut parts, _) = req.into_parts();
    Authenticated::from_request_parts(&mut parts, state).await
  }

  fn basic(user: &str, pass: &str) -> String {
    let encoded = B64.encode(format!("{user}:{pass}"));
    format!("Basic {encoded}")
  }

  #[tokio::test]
  async fn correct_credentials() {
    let state = make_state("secret");
    let req = Request::builder()
      .header(header::AUTHORIZATION, basic("user", "secret"))
      .body(axum::body::Body::empty()).unwrap();
    assert!(extract(req, &state).await.is_ok());
  }

  #[tokio::test]
  async fn wrong_password() {
    let state = make_state("secret");
    let req = Request::builder()
      .header(header::AUTHORIZATION, basic("user", "wrong"))
      .body(axum::body::Body::empty()).unwrap();
    assert!(matches!(extract(req, &state).await, Err(Error::Unauthorized)));
  }

  #[tokio::test]
  async fn missing_header() {
    let state = make_state("secret");
    let req = Request::builder().body(axum::body::Body::empty()).unwrap();
    assert!(matches!(extract(req, &state).await, Err(Error::Unauthorized)));
  }

  #[tokio::test]
  async fn invalid_base64() {
    let state = make_state("secret");
    let req = Request::builder()
      .header(header::AUTHORIZATION, "Basic !!!not-base64!!!")
      .body(axum::body::Body::empty()).unwrap();
    assert!(matches!(extract(req, &state).await, Err(Error::Unauthorized)));
  }
}
