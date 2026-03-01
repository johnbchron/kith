//! REPORT handlers for `addressbook-multiget` and `addressbook-query`.

use axum::response::Response;
use kith_core::{store::ContactStore, subject::SubjectKind};
use uuid::Uuid;

use super::multistatus_response;
use crate::{
  AppState,
  error::Error,
  etag::compute_etag,
  xml::{
    MultistatusBuilder, PropName, Property, ReportKind, ReportRequest,
    parse_report,
  },
};

pub async fn handler<S>(
  state: &AppState<S>,
  ab: &str,
  body: &[u8],
) -> Result<Response, Error>
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let report = parse_report(body)?;
  match report.kind {
    ReportKind::Multiget => multiget(state, ab, &report).await,
    ReportKind::Query => query(state, ab, &report).await,
  }
}

/// `addressbook-multiget`: fetch the requested hrefs and return their data.
async fn multiget<S>(
  state: &AppState<S>,
  ab: &str,
  report: &ReportRequest,
) -> Result<Response, Error>
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let base = &state.config.base_url;
  let want_address_data = report.props.iter().any(|p| *p == PropName::AddressData);
  let want_etag = report.props.iter().any(|p| *p == PropName::GetETag);

  let mut ms = MultistatusBuilder::new();

  for href in &report.hrefs {
    let canonical_href = canonicalize_href(base, ab, href);

    let uid = match uid_from_href(href) {
      Some(uid) => uid,
      None => {
        ms.response(&canonical_href).status_not_found();
        continue;
      }
    };

    let view = state
      .store
      .materialize(uid, None)
      .await
      .map_err(|e| Error::Store(Box::new(e)))?
      .filter(|v| !v.active_facts.is_empty());

    match view {
      None => {
        ms.response(&canonical_href).status_not_found();
      }
      Some(view) => {
        let mut props: Vec<Property> = Vec::new();
        if want_etag {
          props.push(Property::GetETag(compute_etag(&view)));
        }
        if want_address_data {
          let vcard = kith_vcard::serialize(&view)?;
          props.push(Property::AddressData(vcard));
        }
        ms.response(&canonical_href).propstat_ok(&props);
      }
    }
  }

  Ok(multistatus_response(ms.finish()))
}

/// `addressbook-query`: return all contacts in the addressbook.
///
/// We ignore filter conditions for now and return every active contact,
/// which is correct when the client sends an empty or always-true filter.
async fn query<S>(
  state: &AppState<S>,
  ab: &str,
  report: &ReportRequest,
) -> Result<Response, Error>
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let base = &state.config.base_url;
  let want_address_data = report.props.iter().any(|p| *p == PropName::AddressData);
  let want_etag = report.props.iter().any(|p| *p == PropName::GetETag);

  let subjects = state
    .store
    .list_subjects(Some(SubjectKind::Person))
    .await
    .map_err(|e| Error::Store(Box::new(e)))?;

  let mut ms = MultistatusBuilder::new();

  for subject in subjects {
    let view = state
      .store
      .materialize(subject.subject_id, None)
      .await
      .map_err(|e| Error::Store(Box::new(e)))?
      .filter(|v| !v.active_facts.is_empty());

    if let Some(view) = view {
      let href =
        format!("{base}/dav/addressbooks/{ab}/{}.vcf", subject.subject_id);
      let mut props: Vec<Property> = Vec::new();
      if want_etag {
        props.push(Property::GetETag(compute_etag(&view)));
      }
      if want_address_data {
        let vcard = kith_vcard::serialize(&view)?;
        props.push(Property::AddressData(vcard));
      }
      ms.response(&href).propstat_ok(&props);
    }
  }

  Ok(multistatus_response(ms.finish()))
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Parse a UUID from a href like `.../uuid.vcf` or `.../uuid`.
fn uid_from_href(href: &str) -> Option<Uuid> {
  let last = href.trim_end_matches('/').rsplit('/').next()?;
  let s = last.strip_suffix(".vcf").unwrap_or(last);
  Uuid::parse_str(s).ok()
}

/// Produce a canonical absolute href for a resource given the href the client
/// supplied (which may be absolute or relative).
fn canonicalize_href(base: &str, ab: &str, href: &str) -> String {
  if href.starts_with("http://") || href.starts_with("https://") {
    href.to_string()
  } else {
    // Relative: reconstruct from the uid component.
    let last = href.trim_end_matches('/').rsplit('/').next().unwrap_or(href);
    format!("{base}/dav/addressbooks/{ab}/{last}")
  }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn uid_from_absolute_href() {
    let uid = Uuid::new_v4();
    let href = format!(
      "https://contacts.jlewis.sh/dav/addressbooks/personal/{uid}.vcf"
    );
    assert_eq!(uid_from_href(&href), Some(uid));
  }

  #[test]
  fn uid_from_relative_href() {
    let uid = Uuid::new_v4();
    let href = format!("/dav/addressbooks/personal/{uid}.vcf");
    assert_eq!(uid_from_href(&href), Some(uid));
  }

  #[test]
  fn uid_from_garbage_returns_none() {
    assert_eq!(uid_from_href("/not-a-uuid"), None);
  }
}

// Integration tests reusing the test helpers from the parent module live in
// the lib integration tests (lib.rs #[cfg(test)] mod tests).

/// Verify that an `addressbook-multiget` report with a bad status (missing machine)
/// returns a 404 response element, not a 500.
#[cfg(test)]
mod integration {
  use std::sync::Arc;

  use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
  use axum::http::{Request, header};
  use axum::body::Body;
  use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
  use kith_store_sqlite::SqliteStore;
  use rand_core::OsRng;
  use tower::ServiceExt as _;
  use uuid::Uuid;

  use crate::{AppState, ServerConfig, auth::AuthConfig, router};
  use std::path::PathBuf;

  async fn make_state(password: &str) -> AppState<SqliteStore> {
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

  fn auth_header(user: &str, pass: &str) -> String {
    format!("Basic {}", B64.encode(format!("{user}:{pass}")))
  }

  #[tokio::test]
  async fn multiget_nonexistent_returns_207_with_404_response() {
    let state = make_state("secret").await;
    let auth = auth_header("user", "secret");
    let uid = Uuid::new_v4();
    let body = format!(
      r#"<?xml version="1.0"?>
<card:addressbook-multiget xmlns:D="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <D:prop><D:getetag/><card:address-data/></D:prop>
  <D:href>/dav/addressbooks/personal/{uid}.vcf</D:href>
</card:addressbook-multiget>"#
    );

    let req = Request::builder()
      .method("REPORT")
      .uri("/dav/addressbooks/personal")
      .header(header::AUTHORIZATION, auth)
      .header(header::CONTENT_TYPE, "application/xml")
      .body(Body::from(body))
      .unwrap();

    let resp = router(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
      .await
      .unwrap();
    let xml = std::str::from_utf8(&bytes).unwrap();
    assert!(xml.contains("404"), "expected 404 in multistatus: {xml}");
  }

  #[tokio::test]
  async fn multiget_existing_returns_address_data() {
    let state = make_state("secret").await;
    let auth = auth_header("user", "secret");
    let uid = Uuid::new_v4();
    let vcard = format!(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:Bob\r\nEND:VCARD\r\n"
    );

    // PUT the contact first.
    let put_req = Request::builder()
      .method("PUT")
      .uri(format!("/dav/addressbooks/personal/{uid}.vcf"))
      .header(header::AUTHORIZATION, auth.clone())
      .header(header::CONTENT_TYPE, "text/vcard")
      .body(Body::from(vcard.clone()))
      .unwrap();
    router(state.clone()).oneshot(put_req).await.unwrap();

    let body = format!(
      r#"<?xml version="1.0"?>
<card:addressbook-multiget xmlns:D="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <D:prop><D:getetag/><card:address-data/></D:prop>
  <D:href>/dav/addressbooks/personal/{uid}.vcf</D:href>
</card:addressbook-multiget>"#
    );

    let req = Request::builder()
      .method("REPORT")
      .uri("/dav/addressbooks/personal")
      .header(header::AUTHORIZATION, auth)
      .header(header::CONTENT_TYPE, "application/xml")
      .body(Body::from(body))
      .unwrap();

    let resp = router(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
      .await
      .unwrap();
    let xml = std::str::from_utf8(&bytes).unwrap();
    assert!(xml.contains("BEGIN:VCARD"), "missing vcard data: {xml}");
    assert!(xml.contains("200 OK"), "missing 200 propstat: {xml}");
  }
}
