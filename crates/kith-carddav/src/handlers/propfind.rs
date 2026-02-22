//! PROPFIND handlers for principal, home-set, collection, and resource.

use axum::{
  body::Body,
  http::{StatusCode, header},
  response::{IntoResponse, Response},
};
use kith_core::{store::ContactStore, subject::SubjectKind};
use uuid::Uuid;

use crate::{
  AppState,
  error::Error,
  etag::compute_etag,
  xml::{MultistatusBuilder, Property, ResourceType, parse_propfind},
};

const CONTENT_TYPE_MULTISTATUS: &str = "application/xml; charset=utf-8";

fn multistatus_response(body: Vec<u8>) -> Response {
  Response::builder()
    .status(207)
    .header(header::CONTENT_TYPE, CONTENT_TYPE_MULTISTATUS)
    .body(Body::from(body))
    .unwrap()
}

/// PROPFIND /dav/  — principal
pub async fn principal<S>(
  state:    &AppState<S>,
  body:     &[u8],
) -> Result<Response, Error>
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let _req = parse_propfind(body)?;
  let base = &state.config.base_url;
  let href = format!("{base}/dav/");
  let home = format!("{base}/dav/addressbooks/");

  let mut ms = MultistatusBuilder::new();
  ms.response(&href).propstat_ok(&[
    Property::ResourceType(vec![ResourceType::Principal]),
    Property::DisplayName(state.config.auth_username.clone()),
    Property::CurrentUserPrincipal(href.clone()),
    Property::AddressbookHomeSet(home),
  ]);

  Ok(multistatus_response(ms.finish()))
}

/// PROPFIND /dav/addressbooks/  — home set
pub async fn home_set<S>(
  state: &AppState<S>,
  body:  &[u8],
) -> Result<Response, Error>
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let _req = parse_propfind(body)?;
  let base = &state.config.base_url;
  let href = format!("{base}/dav/addressbooks/");

  let mut ms = MultistatusBuilder::new();
  ms.response(&href).propstat_ok(&[
    Property::ResourceType(vec![ResourceType::Collection]),
    Property::DisplayName("Address Books".to_string()),
  ]);

  Ok(multistatus_response(ms.finish()))
}

/// PROPFIND /dav/addressbooks/:ab/  — collection (Depth 0 or 1)
pub async fn collection<S>(
  state: &AppState<S>,
  ab:    &str,
  depth: u8,
  body:  &[u8],
) -> Result<Response, Error>
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  if depth > 1 {
    return Ok((StatusCode::FORBIDDEN, "Depth: infinity not supported").into_response());
  }

  let _req      = parse_propfind(body)?;
  let base      = &state.config.base_url;
  let coll_href = format!("{base}/dav/addressbooks/{ab}/");

  let mut ms = MultistatusBuilder::new();
  ms.response(&coll_href).propstat_ok(&[
    Property::ResourceType(vec![ResourceType::Collection, ResourceType::Addressbook]),
    Property::DisplayName(ab.to_string()),
    Property::SupportedAddressData,
    Property::AddressbookDescription(format!("{ab} address book")),
  ]);

  if depth >= 1 {
    let subjects = state.store
      .list_subjects(Some(SubjectKind::Person))
      .await
      .map_err(|e| Error::Store(Box::new(e)))?;

    for subject in subjects {
      if let Some(view) = state.store
        .materialize(subject.subject_id, None)
        .await
        .map_err(|e| Error::Store(Box::new(e)))?
        .filter(|v| !v.active_facts.is_empty())
      {
        let etag          = compute_etag(&view);
        let vcard         = kith_vcard::serialize(&view)?;
        let content_len   = vcard.len() as u64;
        let resource_href = format!("{base}/dav/addressbooks/{ab}/{}.vcf", subject.subject_id);

        ms.response(&resource_href).propstat_ok(&[
          Property::GetContentType("text/vcard; charset=utf-8".to_string()),
          Property::GetETag(etag),
          Property::GetContentLength(content_len),
        ]);
      }
    }
  }

  Ok(multistatus_response(ms.finish()))
}

/// PROPFIND /dav/addressbooks/:ab/:uid.vcf  — single resource
pub async fn resource<S>(
  state:   &AppState<S>,
  ab:      &str,
  uid_vcf: &str,
  body:    &[u8],
) -> Result<Response, Error>
where
  S: ContactStore + Clone + Send + Sync + 'static,
  S::Error: std::error::Error + Send + Sync + 'static,
{
  let _req = parse_propfind(body)?;
  let uid  = parse_uid(uid_vcf)?;

  let view = state.store
    .materialize(uid, None)
    .await
    .map_err(|e| Error::Store(Box::new(e)))?
    .filter(|v| !v.active_facts.is_empty())
    .ok_or(Error::NotFound)?;

  let etag        = compute_etag(&view);
  let vcard       = kith_vcard::serialize(&view)?;
  let content_len = vcard.len() as u64;
  let base        = &state.config.base_url;
  let href        = format!("{base}/dav/addressbooks/{ab}/{uid_vcf}");

  let last_modified = view
    .active_facts
    .iter()
    .map(|rf| rf.fact.recorded_at)
    .max()
    .unwrap_or(view.as_of);
  let lm_str = last_modified.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

  let mut ms = MultistatusBuilder::new();
  ms.response(&href).propstat_ok(&[
    Property::GetContentType("text/vcard; charset=utf-8".to_string()),
    Property::GetETag(etag),
    Property::GetContentLength(content_len),
    Property::GetLastModified(lm_str),
  ]);

  Ok(multistatus_response(ms.finish()))
}

/// Parse `{uuid}.vcf` → `Uuid`.
pub fn parse_uid(uid_vcf: &str) -> Result<Uuid, Error> {
  let s = uid_vcf.strip_suffix(".vcf").unwrap_or(uid_vcf);
  Uuid::parse_str(s).map_err(|_| Error::BadRequest(format!("invalid UUID: {uid_vcf}")))
}
