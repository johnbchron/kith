//! WebDAV/CardDAV protocol layer for Kith.
//!
//! Exposes an axum [`Router`] backed by a [`DavHandler`] that routes all DAV
//! traffic to [`KithFs`], a [`DavFileSystem`] implementation that maps the
//! kith contact store to a virtual CardDAV address book.

pub mod auth;
pub mod diff;
pub mod error;
pub(crate) mod fs;

use std::{path::PathBuf, sync::Arc};

use auth::{AuthConfig, verify_auth};
use axum::{
  Router,
  body::Body,
  extract::DefaultBodyLimit,
  http::Method,
  response::{IntoResponse, Redirect, Response},
  routing::any,
};
use dav_server::DavHandler;
pub use error::Error;
use fs::KithFs;
use kith_api::api_router;
use kith_core::store::ContactStore;
use serde::Deserialize;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};

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

// ─── DAV-specific route state ─────────────────────────────────────────────────

/// State for the `/dav` subtree.  Separate from `AppState` so the DavHandler
/// does not need to be generic over `S`.
#[derive(Clone)]
struct DavState {
  dav:  DavHandler,
  auth: Arc<AuthConfig>,
}

// ─── Router ───────────────────────────────────────────────────────────────────

/// Build an axum [`Router`] for the CardDAV server.
pub fn router<S>(state: AppState<S>) -> Router
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let kith_fs =
    KithFs::new(Arc::clone(&state.store), state.config.addressbook.clone());

  let dav = DavHandler::builder()
    .filesystem(Box::new(kith_fs))
    .strip_prefix("/dav")
    .build_handler();

  let dav_state = DavState { dav, auth: Arc::clone(&state.auth) };

  // Mount /api routes (they carry the store directly; no DavHandler needed).
  let api = api_router(Arc::clone(&state.store));

  Router::new()
    .route("/.well-known/carddav", any(well_known_handler))
    .route("/.well-known/dav", any(well_known_handler))
    // Catch all /dav paths including the bare /dav root.
    .route("/dav", any(dav_dispatch))
    .route("/dav/{*path}", any(dav_dispatch))
    .with_state(dav_state)
    .nest("/api", api)
    .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
    .layer(
      TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().include_headers(true)),
    )
}

// ─── Handlers ────────────────────────────────────────────────────────────────

async fn well_known_handler() -> Redirect { Redirect::permanent("/dav") }

async fn dav_dispatch(
  axum::extract::State(state): axum::extract::State<DavState>,
  method: Method,
  req: axum::http::Request<Body>,
) -> Response {
  // OPTIONS is always unauthenticated (client discovery).
  if method != Method::OPTIONS {
    if let Err(e) = verify_auth(req.headers(), &state.auth) {
      return e.into_response();
    }
  }

  // Normalise bare (unquoted) ETags in If-Match / If-None-Match.
  // RFC 7232 requires entity-tags to be quoted-strings; some CardDAV clients
  // (and kith's own tests) omit the surrounding `"…"`.
  let req = normalize_etag_headers(req);

  // Hand the request off to dav-server; it handles PROPFIND, REPORT, GET,
  // PUT, DELETE, HEAD, and OPTIONS, including CardDAV-specific protocol.
  let dav_resp = state.dav.handle(req).await;

  // Convert dav_server::body::Body → axum::body::Body.
  let (parts, dav_body) = dav_resp.into_parts();
  Response::from_parts(parts, Body::new(dav_body))
}

/// Wrap bare (unquoted) entity-tag values in double-quotes so they satisfy
/// RFC 7232.  Passes through `*`, weak tags (`W/"…"`), and values that are
/// already quoted.
fn normalize_etag_headers(
  mut req: axum::http::Request<Body>,
) -> axum::http::Request<Body> {
  use axum::http::header::{IF_MATCH, IF_NONE_MATCH};

  for hname in [IF_MATCH, IF_NONE_MATCH] {
    if let Some(val) = req.headers().get(&hname) {
      let s = match val.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => continue,
      };
      if s == "*" || s.starts_with('"') || s.starts_with("W/\"") {
        continue;
      }
      let quoted = format!("\"{s}\"");
      if let Ok(new_val) = axum::http::HeaderValue::from_str(&quoted) {
        req.headers_mut().insert(hname, new_val);
      }
    }
  }
  req
}

// ─── Integration tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use axum::{
    body::Body,
    http::{Request, StatusCode, header},
  };
  use kith_store_sqlite::SqliteStore;
  use tower::ServiceExt as _;
  use uuid::Uuid;

  use super::{
    test_helpers::{auth_header, make_state},
    *,
  };

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

  // ── OPTIONS ────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn options_returns_204_with_dav_header() {
    let state = make_state("secret").await;
    let resp = oneshot_raw(
      state,
      "OPTIONS",
      "/dav/addressbooks/personal",
      vec![],
      "",
    )
    .await;
    // dav-server returns 200 OK for OPTIONS (also valid per RFC 7231).
    assert!(
      resp.status() == StatusCode::NO_CONTENT
        || resp.status() == StatusCode::OK,
      "expected 200 or 204, got {}",
      resp.status()
    );
    let dav_val = resp.headers().get("dav").unwrap().to_str().unwrap();
    assert!(dav_val.contains("addressbook"), "DAV header: {dav_val}");
  }

  // ── PROPFIND collection ────────────────────────────────────────────────────

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
    assert!(xml.contains("personal"), "collection href missing: {xml}");
    assert!(
      xml.contains(&uid.to_string()),
      "resource href missing: {xml}"
    );
  }

  // ── GET ────────────────────────────────────────────────────────────────────

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

  // ── PUT / GET round-trip ──────────────────────────────────────────────────

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

  // ── PUT with If-Match ──────────────────────────────────────────────────────

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

  // ── DELETE ─────────────────────────────────────────────────────────────────

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

  // ── Auth ───────────────────────────────────────────────────────────────────

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

  pub(crate) async fn make_state(
    password: &str,
  ) -> AppState<SqliteStore> {
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
