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

pub use error::Error;

use std::{path::PathBuf, sync::Arc};

use axum::{
  Router,
  body::Body,
  extract::{Request, State},
  http::{Method, StatusCode},
  response::{IntoResponse, Response},
  routing::any,
};
use bytes::Bytes;
use kith_core::store::ContactStore;
use serde::Deserialize;

use auth::{AuthConfig, verify_auth};
use handlers::{delete, get, options, propfind, put};

// ─── Configuration ────────────────────────────────────────────────────────────

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

// ─── Application state ────────────────────────────────────────────────────────

/// Shared state threaded through all axum handlers.
#[derive(Clone)]
pub struct AppState<S: ContactStore> {
  pub store:  Arc<S>,
  pub config: Arc<ServerConfig>,
  pub auth:   Arc<AuthConfig>,
}

// ─── Router ───────────────────────────────────────────────────────────────────

/// Build an axum [`Router`] for the CardDAV server.
pub fn router<S>(state: AppState<S>) -> Router
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  Router::new()
    .route("/dav/",                               any(dav_root_handler::<S>))
    .route("/dav/addressbooks/",                  any(dav_home_handler::<S>))
    .route("/dav/addressbooks/{ab}/",             any(dav_collection_handler::<S>))
    .route("/dav/addressbooks/{ab}/{uid_vcf}",    any(dav_resource_handler::<S>))
    .route("/dav/{*path}",                        any(dav_wildcard_handler::<S>))
    .with_state(state)
}

// ─── Dispatch helpers ────────────────────────────────────────────────────────

/// Return `Some(401 response)` if auth is required but fails.
/// OPTIONS skips auth.
fn check_auth<S>(
  method: &Method,
  req:    &Request<Body>,
  state:  &AppState<S>,
) -> Option<Response>
where
  S: ContactStore + Clone + Send + Sync + 'static,
{
  if method == Method::OPTIONS {
    return None;
  }
  match verify_auth(req.headers(), &state.auth) {
    Ok(_)  => None,
    Err(e) => Some(e.into_response()),
  }
}

async fn collect_body(req: Request<Body>) -> Result<Bytes, Response> {
  axum::body::to_bytes(req.into_body(), 8 * 1024 * 1024)
    .await
    .map_err(|_| {
      (StatusCode::PAYLOAD_TOO_LARGE, "request body too large")
        .into_response()
    })
}

/// GET handler helper: extract ab and uid_vcf from URI path.
fn path_segments_2(uri: &axum::http::Uri) -> Option<(String, String)> {
  let path = uri.path();
  // Expect: /dav/addressbooks/{ab}/{uid_vcf}
  let parts: Vec<&str> = path.trim_end_matches('/').splitn(6, '/').collect();
  if parts.len() >= 5 {
    Some((parts[3].to_string(), parts[4].to_string()))
  } else {
    None
  }
}

fn path_segment_ab(uri: &axum::http::Uri) -> Option<String> {
  let path = uri.path();
  let parts: Vec<&str> = path.trim_end_matches('/').splitn(5, '/').collect();
  if parts.len() >= 4 {
    Some(parts[3].to_string())
  } else {
    None
  }
}

fn depth_from_req(req: &Request<Body>) -> u8 {
  req.headers()
    .get("depth")
    .and_then(|v| v.to_str().ok())
    .and_then(|s| s.parse().ok())
    .unwrap_or(0)
}

// ─── Route handlers ──────────────────────────────────────────────────────────

async fn dav_root_handler<S>(
  State(state): State<AppState<S>>,
  req: Request<Body>,
) -> Response
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let method = req.method().clone();
  if let Some(r) = check_auth(&method, &req, &state) { return r; }
  match method.as_str() {
    "OPTIONS"  => options::handler(),
    "PROPFIND" => {
      let body = match collect_body(req).await { Ok(b) => b, Err(e) => return e };
      propfind::principal(&state, &body).await.into_response_or_err()
    }
    _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
  }
}

async fn dav_home_handler<S>(
  State(state): State<AppState<S>>,
  req: Request<Body>,
) -> Response
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let method = req.method().clone();
  if let Some(r) = check_auth(&method, &req, &state) { return r; }
  match method.as_str() {
    "OPTIONS"  => options::handler(),
    "PROPFIND" => {
      let body = match collect_body(req).await { Ok(b) => b, Err(e) => return e };
      propfind::home_set(&state, &body).await.into_response_or_err()
    }
    _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
  }
}

async fn dav_collection_handler<S>(
  State(state): State<AppState<S>>,
  req: Request<Body>,
) -> Response
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let method = req.method().clone();
  if let Some(r) = check_auth(&method, &req, &state) { return r; }
  let depth = depth_from_req(&req);
  let ab    = match path_segment_ab(req.uri()) {
    Some(ab) => ab,
    None     => return (StatusCode::BAD_REQUEST, "cannot parse path").into_response(),
  };
  match method.as_str() {
    "OPTIONS"  => options::handler(),
    "PROPFIND" => {
      let body = match collect_body(req).await { Ok(b) => b, Err(e) => return e };
      propfind::collection(&state, &ab, depth, &body).await.into_response_or_err()
    }
    _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
  }
}

async fn dav_resource_handler<S>(
  State(state): State<AppState<S>>,
  req: Request<Body>,
) -> Response
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let method = req.method().clone();
  if let Some(r) = check_auth(&method, &req, &state) { return r; }
  let (ab, uid_vcf) = match path_segments_2(req.uri()) {
    Some(p) => p,
    None    => return (StatusCode::BAD_REQUEST, "cannot parse path").into_response(),
  };
  let headers = req.headers().clone();
  match method.as_str() {
    "OPTIONS"  => options::handler(),
    "PROPFIND" => {
      let body = match collect_body(req).await { Ok(b) => b, Err(e) => return e };
      propfind::resource(&state, &ab, &uid_vcf, &body).await.into_response_or_err()
    }
    "GET"  => get::handler(&state, &method, &uid_vcf).await.into_response_or_err(),
    "HEAD" => get::handler(&state, &method, &uid_vcf).await.into_response_or_err(),
    "PUT"  => {
      let body_bytes = match collect_body(req).await { Ok(b) => b, Err(e) => return e };
      let body_str   = match std::str::from_utf8(&body_bytes) {
        Ok(s)  => s,
        Err(_) => return Error::BadRequest("body is not valid UTF-8".to_string()).into_response(),
      };
      put::handler(&state, &headers, &uid_vcf, body_str).await.into_response_or_err()
    }
    "DELETE" => delete::handler(&state, &uid_vcf).await.into_response_or_err(),
    _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
  }
}

async fn dav_wildcard_handler<S>(
  State(_state): State<AppState<S>>,
  req: Request<Body>,
) -> Response
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  if req.method() == Method::OPTIONS {
    options::handler()
  } else {
    StatusCode::NOT_FOUND.into_response()
  }
}

// ─── Helper trait ────────────────────────────────────────────────────────────

trait IntoResponseOrErr {
  fn into_response_or_err(self) -> Response;
}

impl IntoResponseOrErr for Result<Response, Error> {
  fn into_response_or_err(self) -> Response {
    match self {
      Ok(r)  => r,
      Err(e) => e.into_response(),
    }
  }
}

// ─── Integration tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Arc;

  use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
  use rand_core::OsRng;
  use axum::http::{Request, StatusCode, header};
  use base64::Engine as _;
  use base64::engine::general_purpose::STANDARD as B64;
  use kith_store_sqlite::SqliteStore;
  use tower::ServiceExt as _;
  use uuid::Uuid;

  async fn make_state(password: &str) -> AppState<SqliteStore> {
    let store = SqliteStore::open_in_memory().await.unwrap();
    let salt  = SaltString::generate(&mut OsRng);
    let hash  = Argon2::default()
      .hash_password(password.as_bytes(), &salt)
      .unwrap()
      .to_string();

    AppState {
      store: Arc::new(store),
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

  fn auth_header(user: &str, pass: &str) -> String {
    format!("Basic {}", B64.encode(format!("{user}:{pass}")))
  }

  async fn oneshot_raw(
    state:   AppState<SqliteStore>,
    method:  &str,
    uri:     &str,
    headers: Vec<(header::HeaderName, &str)>,
    body:    &str,
  ) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
      builder = builder.header(k, v);
    }
    let req = builder.body(Body::from(body.to_string())).unwrap();
    router(state).oneshot(req).await.unwrap()
  }

  // ── OPTIONS ─────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn options_returns_204_with_dav_header() {
    let state = make_state("secret").await;
    let resp  = oneshot_raw(state, "OPTIONS", "/dav/addressbooks/personal/", vec![], "").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let dav_val = resp.headers().get("dav").unwrap().to_str().unwrap();
    assert!(dav_val.contains("addressbook"), "DAV header: {dav_val}");
  }

  // ── PROPFIND collection ──────────────────────────────────────────────────────

  #[tokio::test]
  async fn propfind_empty_store_returns_207() {
    let state = make_state("secret").await;
    let auth  = auth_header("user", "secret");
    let body  = r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#;
    let resp  = oneshot_raw(
      state,
      "PROPFIND",
      "/dav/addressbooks/personal/",
      vec![
        (header::AUTHORIZATION, auth.as_str()),
        (header::HeaderName::from_static("depth"), "1"),
      ],
      body,
    ).await;
    assert_eq!(resp.status().as_u16(), 207);
  }

  #[tokio::test]
  async fn propfind_with_one_subject_returns_two_responses() {
    let state = make_state("secret").await;
    let auth  = auth_header("user", "secret");
    let uid   = Uuid::new_v4();
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
    ).await;

    let body = r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#;
    let resp = oneshot_raw(
      state,
      "PROPFIND",
      "/dav/addressbooks/personal/",
      vec![
        (header::AUTHORIZATION, auth.as_str()),
        (header::HeaderName::from_static("depth"), "1"),
      ],
      body,
    ).await;
    assert_eq!(resp.status().as_u16(), 207);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let xml   = std::str::from_utf8(&bytes).unwrap();
    assert!(xml.contains("personal/"),         "collection href missing: {xml}");
    assert!(xml.contains(&uid.to_string()),     "resource href missing: {xml}");
  }

  // ── GET ──────────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn get_nonexistent_returns_404() {
    let state = make_state("secret").await;
    let auth  = auth_header("user", "secret");
    let uid   = Uuid::new_v4();
    let resp  = oneshot_raw(
      state,
      "GET",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      "",
    ).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
  }

  // ── PUT / GET round-trip ─────────────────────────────────────────────────────

  #[tokio::test]
  async fn put_creates_and_get_returns_vcard() {
    let state = make_state("secret").await;
    let auth  = auth_header("user", "secret");
    let uid   = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:Test User\r\nEMAIL:test@example.com\r\nEND:VCARD\r\n"
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
    ).await;
    assert_eq!(put_resp.status(), StatusCode::CREATED);
    assert!(put_resp.headers().contains_key(header::ETAG));

    let get_resp = oneshot_raw(
      state,
      "GET",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      "",
    ).await;
    assert_eq!(get_resp.status(), StatusCode::OK);
    let ct = get_resp.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(ct.contains("vcard"), "Content-Type: {ct}");
    let bytes = axum::body::to_bytes(get_resp.into_body(), usize::MAX).await.unwrap();
    let body  = std::str::from_utf8(&bytes).unwrap();
    assert!(body.contains("BEGIN:VCARD"), "body: {body}");
  }

  // ── PUT with If-Match ────────────────────────────────────────────────────────

  #[tokio::test]
  async fn put_with_correct_if_match_returns_204() {
    let state = make_state("secret").await;
    let auth  = auth_header("user", "secret");
    let uid   = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:First\r\nEND:VCARD\r\n"
    );

    let resp1 = oneshot_raw(
      state.clone(),
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      &vcard,
    ).await;
    assert_eq!(resp1.status(), StatusCode::CREATED);
    let etag = resp1.headers().get(header::ETAG).unwrap().to_str().unwrap().to_string();

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
    ).await;
    assert_eq!(resp2.status(), StatusCode::NO_CONTENT);
  }

  #[tokio::test]
  async fn put_with_unquoted_if_match_returns_204() {
    // Some clients send If-Match without the surrounding double-quotes.
    // The server should accept both forms.
    let state = make_state("secret").await;
    let auth  = auth_header("user", "secret");
    let uid   = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:First\r\nEND:VCARD\r\n"
    );

    let resp1 = oneshot_raw(
      state.clone(),
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      &vcard,
    ).await;
    assert_eq!(resp1.status(), StatusCode::CREATED);
    // Strip the surrounding quotes to simulate a bare-ETag client.
    let etag_quoted = resp1.headers().get(header::ETAG).unwrap().to_str().unwrap().to_string();
    let etag_bare   = etag_quoted.trim_matches('"').to_string();

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
    ).await;
    assert_eq!(resp2.status(), StatusCode::NO_CONTENT);
  }

  #[tokio::test]
  async fn put_with_stale_if_match_returns_412() {
    let state = make_state("secret").await;
    let auth  = auth_header("user", "secret");
    let uid   = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:First\r\nEND:VCARD\r\n"
    );

    oneshot_raw(
      state.clone(),
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      &vcard,
    ).await;

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
    ).await;
    assert_eq!(resp2.status(), StatusCode::PRECONDITION_FAILED);
  }

  // ── DELETE ───────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn delete_existing_returns_204_and_get_returns_404() {
    let state = make_state("secret").await;
    let auth  = auth_header("user", "secret");
    let uid   = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:To Delete\r\nEND:VCARD\r\n"
    );

    oneshot_raw(
      state.clone(),
      "PUT",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      &vcard,
    ).await;

    let del_resp = oneshot_raw(
      state.clone(),
      "DELETE",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      "",
    ).await;
    assert_eq!(del_resp.status(), StatusCode::NO_CONTENT);

    let get_resp = oneshot_raw(
      state,
      "GET",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      "",
    ).await;
    assert_eq!(get_resp.status(), StatusCode::NOT_FOUND);
  }

  #[tokio::test]
  async fn delete_nonexistent_returns_404() {
    let state = make_state("secret").await;
    let auth  = auth_header("user", "secret");
    let uid   = Uuid::new_v4();
    let resp  = oneshot_raw(
      state,
      "DELETE",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![(header::AUTHORIZATION, auth.as_str())],
      "",
    ).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
  }

  // ── Auth ─────────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn unauthenticated_requests_return_401() {
    let state = make_state("secret").await;
    let uid   = Uuid::new_v4();

    let resp = oneshot_raw(
      state.clone(),
      "GET",
      &format!("/dav/addressbooks/personal/{uid}.vcf"),
      vec![],
      "",
    ).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(resp.headers().contains_key(header::WWW_AUTHENTICATE));
  }
}
