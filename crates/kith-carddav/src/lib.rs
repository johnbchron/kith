//! WebDAV/CardDAV protocol layer for Kith.
//!
//! Exposes an axum [`Router`] implementing the CardDAV protocol (RFC 6352)
//! backed by any [`ContactStore`].

pub mod auth;
pub mod diff;
pub mod error;
pub mod etag;
pub mod handlers;
pub mod xml;

use std::{path::PathBuf, sync::Arc};

use auth::{AuthConfig, verify_auth};
use axum::{
  Router,
  extract::{DefaultBodyLimit, Path, State},
  http::{HeaderMap, Method, StatusCode},
  response::{IntoResponse, Redirect, Response},
  routing::any,
};
use bytes::Bytes;
pub use error::Error;
use handlers::{delete, get, options, propfind, put, report};
use kith_core::store::ContactStore;
use serde::Deserialize;

// ─── Configuration
// ────────────────────────────────────────────────────────────

/// Runtime server configuration, deserialised from `config.toml`.
#[derive(Deserialize, Clone)]
pub struct ServerConfig {
  pub host:               String,
  pub port:               u16,
  pub base_url:           String,
  pub addressbook:        String,
  pub store_path:         PathBuf,
  pub auth_username:      String,
  pub auth_password_hash: String,
}

// ─── Application state
// ────────────────────────────────────────────────────────

/// Shared state threaded through all axum handlers.
#[derive(Clone)]
pub struct AppState<S: ContactStore> {
  pub store:  Arc<S>,
  pub config: Arc<ServerConfig>,
  pub auth:   Arc<AuthConfig>,
}

// ─── Helpers
// ──────────────────────────────────────────────────────────────────

/// Return `Err(401 response)` for any method other than OPTIONS.
fn require_auth<S>(
  method: &Method,
  headers: &HeaderMap,
  state: &AppState<S>,
) -> Result<(), Response>
where
  S: ContactStore + Clone + Send + Sync + 'static,
{
  if method == Method::OPTIONS {
    return Ok(());
  }
  verify_auth(headers, &state.auth).map_err(|e| e.into_response())
}

/// Parse the `Depth` header as a `u8`, defaulting to `0`.
fn depth(headers: &HeaderMap) -> u8 {
  headers
    .get("depth")
    .and_then(|v| v.to_str().ok())
    .and_then(|s| s.parse().ok())
    .unwrap_or(0)
}

// ─── Router
// ───────────────────────────────────────────────────────────────────

/// Build an axum [`Router`] for the CardDAV server.
pub fn router<S>(state: AppState<S>) -> Router
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  Router::new()
    .route("/.well-known/carddav", any(well_known_dav_handler))
    .route("/.well-known/dav", any(well_known_dav_handler))
    .route("/dav", any(dav_root_handler::<S>))
    .route("/dav/addressbooks", any(dav_home_handler::<S>))
    .route("/dav/addressbooks/{ab}", any(dav_collection_handler::<S>))
    .route(
      "/dav/addressbooks/{ab}/{uid_vcf}",
      any(dav_resource_handler::<S>),
    )
    .route("/dav/{*path}", any(dav_wildcard_handler))
    .with_state(state)
    .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
}

// ─── Route handlers ──────────────────────────────────────────────────────────

async fn dav_root_handler<S>(
  State(state): State<AppState<S>>,
  method: Method,
  headers: HeaderMap,
  body: Bytes,
) -> Response
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  if let Err(r) = require_auth(&method, &headers, &state) {
    return r;
  }
  match method.as_str() {
    "OPTIONS" => options::handler(),
    "PROPFIND" => propfind::principal(&state, &body)
      .await
      .into_response_or_err(),
    _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
  }
}

async fn dav_home_handler<S>(
  State(state): State<AppState<S>>,
  method: Method,
  headers: HeaderMap,
  body: Bytes,
) -> Response
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  if let Err(r) = require_auth(&method, &headers, &state) {
    return r;
  }
  match method.as_str() {
    "OPTIONS" => options::handler(),
    "PROPFIND" => propfind::home_set(&state, depth(&headers), &body)
      .await
      .into_response_or_err(),
    _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
  }
}

async fn dav_collection_handler<S>(
  State(state): State<AppState<S>>,
  Path(ab): Path<String>,
  method: Method,
  headers: HeaderMap,
  body: Bytes,
) -> Response
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  if let Err(r) = require_auth(&method, &headers, &state) {
    return r;
  }
  match method.as_str() {
    "OPTIONS" => options::handler(),
    "PROPFIND" => propfind::collection(&state, &ab, depth(&headers), &body)
      .await
      .into_response_or_err(),
    "REPORT" => report::handler(&state, &ab, &body)
      .await
      .into_response_or_err(),
    _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
  }
}

async fn dav_resource_handler<S>(
  State(state): State<AppState<S>>,
  Path((ab, uid_vcf)): Path<(String, String)>,
  method: Method,
  headers: HeaderMap,
  body: Bytes,
) -> Response
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  if let Err(r) = require_auth(&method, &headers, &state) {
    return r;
  }
  match method.as_str() {
    "OPTIONS" => options::handler(),
    "GET" | "HEAD" => get::handler(&state, &method, &uid_vcf)
      .await
      .into_response_or_err(),
    "PUT" => {
      let body_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
          return Error::BadRequest("body is not valid UTF-8".to_string())
            .into_response()
        }
      };
      put::handler(&state, &headers, &uid_vcf, body_str)
        .await
        .into_response_or_err()
    }
    "DELETE" => delete::handler(&state, &uid_vcf)
      .await
      .into_response_or_err(),
    "PROPFIND" => propfind::resource(&state, &ab, &uid_vcf, &body)
      .await
      .into_response_or_err(),
    _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
  }
}

async fn dav_wildcard_handler(method: Method) -> Response {
  if method == Method::OPTIONS {
    options::handler()
  } else {
    StatusCode::NOT_FOUND.into_response()
  }
}

async fn well_known_dav_handler() -> Redirect {
  Redirect::permanent("/dav")
}

// ─── Helper trait ────────────────────────────────────────────────────────────

trait IntoResponseOrErr {
  fn into_response_or_err(self) -> Response;
}

impl IntoResponseOrErr for Result<Response, Error> {
  fn into_response_or_err(self) -> Response {
    match self {
      Ok(r) => r,
      Err(e) => e.into_response(),
    }
  }
}

// ─── Integration tests
// ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use axum::body::Body;
  use axum::http::{Request, StatusCode, header};
  use kith_store_sqlite::SqliteStore;
  use tower::ServiceExt as _;
  use uuid::Uuid;

  use super::{test_helpers::{auth_header, make_state}, *};

  async fn oneshot_raw(
    state: AppState<SqliteStore>,
    method: &str,
    uri: &str,
    headers: Vec<(header::HeaderName, &str)>,
    body: &str,
  ) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
      builder = builder.header(k, v);
    }
    let req = builder.body(Body::from(body.to_string())).unwrap();
    router(state).oneshot(req).await.unwrap()
  }

  // ── OPTIONS
  // ─────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn options_returns_204_with_dav_header() {
    let state = make_state("secret").await;
    let resp =
      oneshot_raw(state, "OPTIONS", "/dav/addressbooks/personal", vec![], "")
        .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let dav_val = resp.headers().get("dav").unwrap().to_str().unwrap();
    assert!(dav_val.contains("addressbook"), "DAV header: {dav_val}");
  }

  // ── PROPFIND collection
  // ──────────────────────────────────────────────────────

  #[tokio::test]
  async fn propfind_empty_store_returns_207() {
    let state = make_state("secret").await;
    let auth = auth_header("user", "secret");
    let body = r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#;
    let resp = oneshot_raw(
      state,
      "PROPFIND",
      "/dav/addressbooks/personal",
      vec![
        (header::AUTHORIZATION, auth.as_str()),
        (header::HeaderName::from_static("depth"), "1"),
      ],
      body,
    )
    .await;
    assert_eq!(resp.status().as_u16(), 207);
  }

  #[tokio::test]
  async fn propfind_with_one_subject_returns_two_responses() {
    let state = make_state("secret").await;
    let auth = auth_header("user", "secret");
    let uid = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:Alice\r\nEND:VCARD\r\n"
    );

    oneshot_raw(
      state.clone(),
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![
        (header::AUTHORIZATION, auth.as_str()),
        (header::CONTENT_TYPE, "text/vcard"),
      ],
      &vcard,
    )
    .await;

    let body = r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#;
    let resp = oneshot_raw(
      state,
      "PROPFIND",
      "/dav/addressbooks/personal",
      vec![
        (header::AUTHORIZATION, auth.as_str()),
        (header::HeaderName::from_static("depth"), "1"),
      ],
      body,
    )
    .await;
    assert_eq!(resp.status().as_u16(), 207);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
      .await
      .unwrap();
    let xml = std::str::from_utf8(&bytes).unwrap();
    assert!(xml.contains("personal/"), "collection href missing: {xml}");
    assert!(
      xml.contains(&uid.to_string()),
      "resource href missing: {xml}"
    );
  }

  // ── GET ──────────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn get_nonexistent_returns_404() {
    let state = make_state("secret").await;
    let auth = auth_header("user", "secret");
    let uid = Uuid::new_v4();
    let resp = oneshot_raw(
      state,
      "GET",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      "",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
  }

  // ── PUT / GET round-trip
  // ─────────────────────────────────────────────────────

  #[tokio::test]
  async fn put_creates_and_get_returns_vcard() {
    let state = make_state("secret").await;
    let auth = auth_header("user", "secret");
    let uid = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:Test \
       User\r\nEMAIL:test@example.com\r\nEND:VCARD\r\n"
    );

    let put_resp = oneshot_raw(
      state.clone(),
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![
        (header::AUTHORIZATION, auth.as_str()),
        (header::CONTENT_TYPE, "text/vcard"),
      ],
      &vcard,
    )
    .await;
    assert_eq!(put_resp.status(), StatusCode::CREATED);
    assert!(put_resp.headers().contains_key(header::ETAG));

    let get_resp = oneshot_raw(
      state,
      "GET",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      "",
    )
    .await;
    assert_eq!(get_resp.status(), StatusCode::OK);
    let ct = get_resp
      .headers()
      .get(header::CONTENT_TYPE)
      .unwrap()
      .to_str()
      .unwrap();
    assert!(ct.contains("vcard"), "Content-Type: {ct}");
    let bytes = axum::body::to_bytes(get_resp.into_body(), usize::MAX)
      .await
      .unwrap();
    let body = std::str::from_utf8(&bytes).unwrap();
    assert!(body.contains("BEGIN:VCARD"), "body: {body}");
  }

  // ── PUT with If-Match
  // ────────────────────────────────────────────────────────

  #[tokio::test]
  async fn put_with_correct_if_match_returns_204() {
    let state = make_state("secret").await;
    let auth = auth_header("user", "secret");
    let uid = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:First\r\nEND:VCARD\r\n"
    );

    let resp1 = oneshot_raw(
      state.clone(),
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      &vcard,
    )
    .await;
    assert_eq!(resp1.status(), StatusCode::CREATED);
    let etag = resp1
      .headers()
      .get(header::ETAG)
      .unwrap()
      .to_str()
      .unwrap()
      .to_string();

    let vcard2 = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:Updated\r\nEND:VCARD\r\n"
    );
    let resp2 = oneshot_raw(
      state,
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![
        (header::AUTHORIZATION, auth.as_str()),
        (header::IF_MATCH, etag.as_str()),
      ],
      &vcard2,
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::NO_CONTENT);
  }

  #[tokio::test]
  async fn put_with_unquoted_if_match_returns_204() {
    // Some clients send If-Match without the surrounding double-quotes.
    // The server should accept both forms.
    let state = make_state("secret").await;
    let auth = auth_header("user", "secret");
    let uid = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:First\r\nEND:VCARD\r\n"
    );

    let resp1 = oneshot_raw(
      state.clone(),
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      &vcard,
    )
    .await;
    assert_eq!(resp1.status(), StatusCode::CREATED);
    // Strip the surrounding quotes to simulate a bare-ETag client.
    let etag_quoted = resp1
      .headers()
      .get(header::ETAG)
      .unwrap()
      .to_str()
      .unwrap()
      .to_string();
    let etag_bare = etag_quoted.trim_matches('"').to_string();

    let vcard2 = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:Updated\r\nEND:VCARD\r\n"
    );
    let resp2 = oneshot_raw(
      state,
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![
        (header::AUTHORIZATION, auth.as_str()),
        (header::IF_MATCH, etag_bare.as_str()),
      ],
      &vcard2,
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::NO_CONTENT);
  }

  #[tokio::test]
  async fn put_with_stale_if_match_returns_412() {
    let state = make_state("secret").await;
    let auth = auth_header("user", "secret");
    let uid = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:First\r\nEND:VCARD\r\n"
    );

    oneshot_raw(
      state.clone(),
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      &vcard,
    )
    .await;

    let vcard2 = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:Updated\r\nEND:VCARD\r\n"
    );
    let resp2 = oneshot_raw(
      state,
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![
        (header::AUTHORIZATION, auth.as_str()),
        (header::IF_MATCH, "\"stale-etag\""),
      ],
      &vcard2,
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::PRECONDITION_FAILED);
  }

  // ── DELETE ───────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn delete_existing_returns_204_and_get_returns_404() {
    let state = make_state("secret").await;
    let auth = auth_header("user", "secret");
    let uid = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:To \
       Delete\r\nEND:VCARD\r\n"
    );

    oneshot_raw(
      state.clone(),
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      &vcard,
    )
    .await;

    let del_resp = oneshot_raw(
      state.clone(),
      "DELETE",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      "",
    )
    .await;
    assert_eq!(del_resp.status(), StatusCode::NO_CONTENT);

    let get_resp = oneshot_raw(
      state,
      "GET",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      "",
    )
    .await;
    assert_eq!(get_resp.status(), StatusCode::NOT_FOUND);
  }

  #[tokio::test]
  async fn delete_nonexistent_returns_404() {
    let state = make_state("secret").await;
    let auth = auth_header("user", "secret");
    let uid = Uuid::new_v4();
    let resp = oneshot_raw(
      state,
      "DELETE",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      "",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
  }

  // ── Auth ─────────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn unauthenticated_requests_return_401() {
    let state = make_state("secret").await;
    let uid = Uuid::new_v4();

    let resp = oneshot_raw(
      state.clone(),
      "GET",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![],
      "",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(resp.headers().contains_key(header::WWW_AUTHENTICATE));
  }
}

// ─── Shared test helpers ──────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod test_helpers {
  use std::{path::PathBuf, sync::Arc};

  use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
  use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
  use kith_store_sqlite::SqliteStore;
  use rand_core::OsRng;

  use crate::{AppState, ServerConfig, auth::AuthConfig};

  pub(crate) async fn make_state(password: &str) -> AppState<SqliteStore> {
    let store = SqliteStore::open_in_memory().await.unwrap();
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
      .hash_password(password.as_bytes(), &salt)
      .unwrap()
      .to_string();
    AppState {
      store:  Arc::new(store),
      config: Arc::new(ServerConfig {
        host:               "127.0.0.1".to_string(),
        port:               5232,
        base_url:           "http://localhost:5232".to_string(),
        addressbook:        "personal".to_string(),
        store_path:         PathBuf::from(":memory:"),
        auth_username:      "user".to_string(),
        auth_password_hash: hash.clone(),
      }),
      auth:   Arc::new(AuthConfig {
        username:      "user".to_string(),
        password_hash: hash,
      }),
    }
  }

  pub(crate) fn auth_header(user: &str, pass: &str) -> String {
    format!("Basic {}", B64.encode(format!("{user}:{pass}")))
  }
}
