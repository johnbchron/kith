//! [`DavFileSystem`] implementation that maps the kith contact store to a
//! virtual CardDAV filesystem.
//!
//! Virtual path layout (after the `/dav` prefix is stripped by [`DavHandler`]):
//!
//! ```text
//! /                                 root collection
//! /addressbooks/                    address book home
//! /addressbooks/{ab}/               CardDAV address book collection
//! /addressbooks/{ab}/{uuid}.vcf     vCard resource
//! ```

use std::{
  io::SeekFrom,
  sync::Arc,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use bytes::{Buf, Bytes};
use dav_server::{
  davpath::DavPath,
  fs::{
    DavDirEntry, DavFile, DavFileSystem, DavMetaData, DavProp, FsError,
    FsFuture, FsResult, FsStream, OpenOptions, ReadDirMeta,
  },
};
use futures_util::{future, stream};
use kith_core::{store::ContactStore, subject::SubjectKind};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::diff;

// ─── Namespace UUID ───────────────────────────────────────────────────────────

/// Namespace for deriving stable v5 UUIDs from non-UUID path components.
/// Must never change once deployed.
const UID_NAMESPACE: Uuid =
  Uuid::from_u128(0x6b697468_0000_5000_8000_000000000000);

// ─── ETag ─────────────────────────────────────────────────────────────────────

/// Compute a strong ETag for a [`ContactView`] (SHA-256 over sorted
/// `(fact_id, recorded_at)` pairs). Stable regardless of insertion order.
fn compute_etag(view: &kith_core::lifecycle::ContactView) -> String {
  let mut pairs: Vec<(Uuid, i64)> = view
    .active_facts
    .iter()
    .map(|rf| (rf.fact.fact_id, rf.fact.recorded_at.timestamp_micros()))
    .collect();
  pairs.sort_by_key(|(id, _)| *id);

  let mut h = Sha256::new();
  h.update(view.subject.subject_id.as_bytes());
  for (id, ts) in &pairs {
    h.update(id.as_bytes());
    h.update(ts.to_le_bytes());
  }
  // Return the bare opaque tag (no surrounding quotes).
  // dav-server wraps it in `"..."` when it sets the ETag response header
  // and strips quotes when comparing against If-Match values.
  hex::encode(h.finalize())
}

// ─── Path classification ──────────────────────────────────────────────────────

#[derive(Debug)]
enum PathKind {
  Root,
  AddressbookHome,
  Collection,
  Resource { uid: Uuid },
  Unknown,
}

fn classify(path: &DavPath, ab: &str) -> PathKind {
  // as_url_string() returns the path component from the original URL; the
  // DavHandler has already stripped the configured strip_prefix.
  let s = path.as_url_string();
  let s = s.trim_end_matches('/');

  match s {
    "" | "/" => PathKind::Root,
    "/addressbooks" => PathKind::AddressbookHome,
    p if p == format!("/addressbooks/{ab}") => PathKind::Collection,
    p => {
      let prefix = format!("/addressbooks/{ab}/");
      if let Some(rest) = p.strip_prefix(&prefix) {
        let uid_str = rest.strip_suffix(".vcf").unwrap_or(rest);
        // Decode any %-encoded chars in case the client uses them.
        let decoded = percent_decode(uid_str);
        let uid = Uuid::parse_str(&decoded).unwrap_or_else(|_| {
          Uuid::new_v5(&UID_NAMESPACE, decoded.as_bytes())
        });
        PathKind::Resource { uid }
      } else {
        PathKind::Unknown
      }
    }
  }
}

fn percent_decode(s: &str) -> String {
  // Minimal percent-decode: only handles %XX sequences.
  let mut out = String::with_capacity(s.len());
  let bytes = s.as_bytes();
  let mut i = 0;
  while i < bytes.len() {
    if bytes[i] == b'%' && i + 2 < bytes.len() {
      if let Ok(b) = u8::from_str_radix(
        std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
        16,
      ) {
        out.push(b as char);
        i += 3;
        continue;
      }
    }
    out.push(bytes[i] as char);
    i += 1;
  }
  out
}

// ─── KithMetaData ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KithMetaData {
  is_dir:              bool,
  is_addressbook_flag: bool,
  len:                 u64,
  modified:            SystemTime,
  etag_val:            Option<String>,
}

impl KithMetaData {
  fn dir(modified: SystemTime) -> Self {
    Self {
      is_dir:              true,
      is_addressbook_flag: false,
      len:                 0,
      modified,
      etag_val:            None,
    }
  }

  fn addressbook(modified: SystemTime) -> Self {
    Self {
      is_dir:              true,
      is_addressbook_flag: true,
      len:                 0,
      modified,
      etag_val:            None,
    }
  }

  fn resource(len: u64, modified: SystemTime, etag: String) -> Self {
    Self {
      is_dir:              false,
      is_addressbook_flag: false,
      len,
      modified,
      etag_val:            Some(etag),
    }
  }
}

impl DavMetaData for KithMetaData {
  fn len(&self) -> u64 { self.len }
  fn modified(&self) -> FsResult<SystemTime> { Ok(self.modified) }
  fn is_dir(&self) -> bool { self.is_dir }
  fn is_addressbook(&self, _path: &DavPath) -> bool {
    self.is_addressbook_flag
  }
  fn etag(&self) -> Option<String> { self.etag_val.clone() }
}

// ─── KithDirEntry ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct KithDirEntry {
  name: Vec<u8>,
  meta: KithMetaData,
}

impl DavDirEntry for KithDirEntry {
  fn name(&self) -> Vec<u8> { self.name.clone() }

  fn metadata(&self) -> FsFuture<'_, Box<dyn DavMetaData>> {
    let m: Box<dyn DavMetaData> = Box::new(self.meta.clone());
    Box::pin(future::ready(Ok(m)))
  }
}

// ─── KithReadFile ─────────────────────────────────────────────────────────────

#[derive(Debug)]
struct KithReadFile {
  content: Bytes,
  pos:     usize,
  meta:    KithMetaData,
}

impl DavFile for KithReadFile {
  fn metadata(&mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
    let m: Box<dyn DavMetaData> = Box::new(self.meta.clone());
    Box::pin(future::ready(Ok(m)))
  }

  fn read_bytes(&mut self, count: usize) -> FsFuture<'_, Bytes> {
    let start = self.pos;
    let end = (self.pos + count).min(self.content.len());
    self.pos = end;
    Box::pin(future::ready(Ok(self.content.slice(start..end))))
  }

  fn write_bytes(&mut self, _buf: Bytes) -> FsFuture<'_, ()> {
    Box::pin(future::ready(Err(FsError::Forbidden)))
  }

  fn write_buf(
    &mut self,
    _buf: Box<dyn Buf + Send>,
  ) -> FsFuture<'_, ()> {
    Box::pin(future::ready(Err(FsError::Forbidden)))
  }

  fn flush(&mut self) -> FsFuture<'_, ()> {
    Box::pin(future::ready(Ok(())))
  }

  fn seek(&mut self, pos: SeekFrom) -> FsFuture<'_, u64> {
    let len = self.content.len() as u64;
    let new_pos = match pos {
      SeekFrom::Start(n) => n.min(len),
      SeekFrom::End(n) => ((len as i64) + n).max(0) as u64,
      SeekFrom::Current(n) => ((self.pos as i64) + n).max(0) as u64,
    };
    self.pos = new_pos as usize;
    Box::pin(future::ready(Ok(new_pos)))
  }
}

// ─── KithWriteFile ────────────────────────────────────────────────────────────

struct KithWriteFile<S: ContactStore + Send + Sync + 'static> {
  store:      Arc<S>,
  subject_id: Uuid,
  buf:        Vec<u8>,
}

impl<S: ContactStore + Send + Sync + 'static> std::fmt::Debug
  for KithWriteFile<S>
{
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("KithWriteFile")
      .field("subject_id", &self.subject_id)
      .field("buf_len", &self.buf.len())
      .finish()
  }
}

impl<S: ContactStore + Send + Sync + 'static> DavFile for KithWriteFile<S>
where
  S::Error: std::error::Error + Send + Sync + 'static,
{
  fn metadata(&mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
    // dav-server calls this after flush() to obtain the ETag for the PUT
    // response header.  Query the store so we return the real hash rather
    // than an empty placeholder.
    let store = Arc::clone(&self.store);
    let subject_id = self.subject_id;
    let buf_len = self.buf.len() as u64; // pre-flush size estimate

    Box::pin(async move {
      match store.materialize(subject_id, None).await {
        Ok(Some(view)) if !view.active_facts.is_empty() => {
          let etag = compute_etag(&view);
          match kith_vcard::serialize(&view) {
            Ok(vcard) => {
              let len = vcard.len() as u64;
              let modified = view
                .active_facts
                .iter()
                .map(|rf| rf.fact.recorded_at.timestamp_micros())
                .max()
                .map(|us| UNIX_EPOCH + Duration::from_micros(us.max(0) as u64))
                .unwrap_or(UNIX_EPOCH);
              let m: Box<dyn DavMetaData> =
                Box::new(KithMetaData::resource(len, modified, etag));
              Ok(m)
            }
            Err(_) => Err(FsError::GeneralFailure),
          }
        }
        // Pre-flush or subject has no active facts yet.
        _ => {
          let m: Box<dyn DavMetaData> = Box::new(KithMetaData::resource(
            buf_len,
            SystemTime::now(),
            String::new(),
          ));
          Ok(m)
        }
      }
    })
  }

  fn read_bytes(&mut self, _count: usize) -> FsFuture<'_, Bytes> {
    Box::pin(future::ready(Err(FsError::Forbidden)))
  }

  fn write_bytes(&mut self, buf: Bytes) -> FsFuture<'_, ()> {
    self.buf.extend_from_slice(&buf);
    Box::pin(future::ready(Ok(())))
  }

  fn write_buf(&mut self, mut buf: Box<dyn Buf + Send>) -> FsFuture<'_, ()> {
    while buf.has_remaining() {
      let chunk = buf.chunk().to_vec();
      let len = chunk.len();
      self.buf.extend_from_slice(&chunk);
      buf.advance(len);
    }
    Box::pin(future::ready(Ok(())))
  }

  fn flush(&mut self) -> FsFuture<'_, ()> {
    // Move data out of self — the async block owns everything it needs.
    let buf = std::mem::take(&mut self.buf);
    let store = Arc::clone(&self.store);
    let subject_id = self.subject_id;

    Box::pin(async move {
      let vcard_str =
        String::from_utf8(buf).map_err(|_| FsError::GeneralFailure)?;

      let current_view = store
        .materialize(subject_id, None)
        .await
        .map_err(|_| FsError::GeneralFailure)?;

      let result =
        diff::diff(&vcard_str, subject_id, "carddav-put", current_view.as_ref())
          .map_err(|_| FsError::GeneralFailure)?;

      for nf in result.new_facts {
        store
          .record_fact(nf)
          .await
          .map_err(|_| FsError::GeneralFailure)?;
      }
      for (old_id, replacement) in result.supersessions {
        if let Err(e) = store.supersede(old_id, replacement).await {
          tracing::debug!(
            %subject_id, %old_id,
            error = %e,
            "supersede skipped (already applied concurrently?)"
          );
        }
      }
      for fact_id in result.retractions {
        if let Err(e) = store
          .retract(fact_id, Some("Superseded by CardDAV PUT".to_string()))
          .await
        {
          tracing::debug!(
            %subject_id, %fact_id,
            error = %e,
            "retract skipped (already applied concurrently?)"
          );
        }
      }

      Ok(())
    })
  }

  fn seek(&mut self, _pos: SeekFrom) -> FsFuture<'_, u64> {
    Box::pin(future::ready(Err(FsError::NotImplemented)))
  }
}

// ─── KithFs ───────────────────────────────────────────────────────────────────

/// A [`DavFileSystem`] that maps kith's contact store to a virtual CardDAV
/// address book. Clone is cheap (inner store is `Arc`-wrapped).
#[derive(Clone)]
pub struct KithFs<S: ContactStore + Clone + Send + Sync + 'static> {
  store:       Arc<S>,
  addressbook: String,
}

impl<S: ContactStore + Clone + Send + Sync + 'static> KithFs<S> {
  pub fn new(store: Arc<S>, addressbook: String) -> Self {
    Self { store, addressbook }
  }

  /// Materialize a [`ContactView`] and serialize it as a vCard string.
  /// Returns `FsError::NotFound` if the subject does not exist or has no
  /// active facts.
  async fn load_vcard(
    &self,
    uid: Uuid,
  ) -> FsResult<(kith_core::lifecycle::ContactView, String)> {
    let view = self
      .store
      .materialize(uid, None)
      .await
      .map_err(|_| FsError::GeneralFailure)?
      .filter(|v| !v.active_facts.is_empty())
      .ok_or(FsError::NotFound)?;

    let vcard =
      kith_vcard::serialize(&view).map_err(|_| FsError::GeneralFailure)?;
    Ok((view, vcard))
  }

  /// Build [`KithMetaData`] for an existing vCard resource.
  async fn resource_meta(&self, uid: Uuid) -> FsResult<KithMetaData> {
    let (view, vcard) = self.load_vcard(uid).await?;
    let etag = compute_etag(&view);
    let len = vcard.len() as u64;
    let modified = view
      .active_facts
      .iter()
      .map(|rf| rf.fact.recorded_at.timestamp_micros())
      .max()
      .map(|us| UNIX_EPOCH + Duration::from_micros(us.max(0) as u64))
      .unwrap_or(UNIX_EPOCH);
    Ok(KithMetaData::resource(len, modified, etag))
  }
}

impl<S: ContactStore + Clone + Send + Sync + 'static> DavFileSystem
  for KithFs<S>
where
  S::Error: std::error::Error + Send + Sync + 'static,
{
  // ── metadata ──────────────────────────────────────────────────────────────

  fn metadata<'a>(
    &'a self,
    path: &'a DavPath,
  ) -> FsFuture<'a, Box<dyn DavMetaData>> {
    Box::pin(async move {
      let now = SystemTime::now();
      let meta: Box<dyn DavMetaData> = match classify(path, &self.addressbook)
      {
        PathKind::Root | PathKind::AddressbookHome => {
          Box::new(KithMetaData::dir(now))
        }
        PathKind::Collection => Box::new(KithMetaData::addressbook(now)),
        PathKind::Resource { uid } => {
          Box::new(self.resource_meta(uid).await?)
        }
        PathKind::Unknown => return Err(FsError::NotFound),
      };
      Ok(meta)
    })
  }

  // ── read_dir ──────────────────────────────────────────────────────────────

  fn read_dir<'a>(
    &'a self,
    path: &'a DavPath,
    _meta: ReadDirMeta,
  ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
    Box::pin(async move {
      match classify(path, &self.addressbook) {
        PathKind::Root => {
          let entry: Box<dyn DavDirEntry> = Box::new(KithDirEntry {
            name: b"addressbooks".to_vec(),
            meta: KithMetaData::dir(SystemTime::now()),
          });
          Ok(Box::pin(stream::iter(vec![Ok(entry)]))
            as FsStream<Box<dyn DavDirEntry>>)
        }

        PathKind::AddressbookHome => {
          let entry: Box<dyn DavDirEntry> = Box::new(KithDirEntry {
            name: self.addressbook.as_bytes().to_vec(),
            meta: KithMetaData::addressbook(SystemTime::now()),
          });
          Ok(Box::pin(stream::iter(vec![Ok(entry)]))
            as FsStream<Box<dyn DavDirEntry>>)
        }

        PathKind::Collection => {
          let subjects = self
            .store
            .list_subjects(Some(SubjectKind::Person))
            .await
            .map_err(|_| FsError::GeneralFailure)?;

          let mut entries: Vec<FsResult<Box<dyn DavDirEntry>>> =
            Vec::with_capacity(subjects.len());

          for subject in subjects {
            match self.resource_meta(subject.subject_id).await {
              Ok(meta) => {
                let name =
                  format!("{}.vcf", subject.subject_id).into_bytes();
                entries.push(Ok(Box::new(KithDirEntry { name, meta })));
              }
              Err(FsError::NotFound) => {
                // Subject exists but has no active facts — invisible.
              }
              Err(e) => entries.push(Err(e)),
            }
          }

          Ok(Box::pin(stream::iter(entries))
            as FsStream<Box<dyn DavDirEntry>>)
        }

        _ => Err(FsError::Forbidden),
      }
    })
  }

  // ── open ──────────────────────────────────────────────────────────────────

  fn open<'a>(
    &'a self,
    path: &'a DavPath,
    options: OpenOptions,
  ) -> FsFuture<'a, Box<dyn DavFile>> {
    Box::pin(async move {
      let uid = match classify(path, &self.addressbook) {
        PathKind::Resource { uid } => uid,
        _ => return Err(FsError::NotFound),
      };

      if options.write || options.append {
        // ── Write path ──────────────────────────────────────────────────
        let subject_exists = self
          .store
          .get_subject(uid)
          .await
          .map_err(|_| FsError::GeneralFailure)?
          .is_some();

        if !subject_exists {
          if !(options.create || options.create_new) {
            return Err(FsError::NotFound);
          }
          self
            .store
            .add_subject_with_id(uid, SubjectKind::Person)
            .await
            .map_err(|_| FsError::GeneralFailure)?;
        } else if options.create_new {
          // create_new means "fail if already exists"
          return Err(FsError::Exists);
        }

        let file: Box<dyn DavFile> = Box::new(KithWriteFile {
          store:      Arc::clone(&self.store),
          subject_id: uid,
          buf:        Vec::new(),
        });
        Ok(file)
      } else {
        // ── Read path ───────────────────────────────────────────────────
        let (view, vcard) = self.load_vcard(uid).await?;
        let etag = compute_etag(&view);
        let content = Bytes::from(vcard.into_bytes());
        let modified = view
          .active_facts
          .iter()
          .map(|rf| rf.fact.recorded_at.timestamp_micros())
          .max()
          .map(|us| UNIX_EPOCH + Duration::from_micros(us.max(0) as u64))
          .unwrap_or(UNIX_EPOCH);
        let meta =
          KithMetaData::resource(content.len() as u64, modified, etag);

        let file: Box<dyn DavFile> =
          Box::new(KithReadFile { content, pos: 0, meta });
        Ok(file)
      }
    })
  }

  // ── remove_file ───────────────────────────────────────────────────────────

  fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
    Box::pin(async move {
      let uid = match classify(path, &self.addressbook) {
        PathKind::Resource { uid } => uid,
        _ => return Err(FsError::NotFound),
      };

      let facts = self
        .store
        .get_facts(uid, None, false)
        .await
        .map_err(|_| FsError::GeneralFailure)?;

      if facts.is_empty() {
        return Err(FsError::NotFound);
      }

      for rf in facts {
        self
          .store
          .retract(
            rf.fact.fact_id,
            Some("Deleted via CardDAV".to_string()),
          )
          .await
          .map_err(|_| FsError::GeneralFailure)?;
      }

      Ok(())
    })
  }

  // ── directory mutations (forbidden) ───────────────────────────────────────

  fn create_dir<'a>(&'a self, _path: &'a DavPath) -> FsFuture<'a, ()> {
    Box::pin(future::ready(Err(FsError::Forbidden)))
  }

  fn remove_dir<'a>(&'a self, _path: &'a DavPath) -> FsFuture<'a, ()> {
    Box::pin(future::ready(Err(FsError::Forbidden)))
  }

  // ── DAV properties (card:address-data) ────────────────────────────────────

  /// Signal that resources expose `card:address-data` as a custom property.
  fn have_props<'a>(
    &'a self,
    path: &'a DavPath,
  ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>>
  {
    let is_resource =
      matches!(classify(path, &self.addressbook), PathKind::Resource { .. });
    Box::pin(future::ready(is_resource))
  }

  fn get_props<'a>(
    &'a self,
    path: &'a DavPath,
    do_content: bool,
  ) -> FsFuture<'a, Vec<DavProp>> {
    Box::pin(async move {
      let uid = match classify(path, &self.addressbook) {
        PathKind::Resource { uid } => uid,
        _ => return Ok(vec![]),
      };

      if !do_content {
        return Ok(vec![address_data_prop(None)]);
      }

      let (_, vcard) = self.load_vcard(uid).await?;
      Ok(vec![address_data_prop(Some(&vcard))])
    })
  }

  fn get_prop<'a>(
    &'a self,
    path: &'a DavPath,
    prop: DavProp,
  ) -> FsFuture<'a, Vec<u8>> {
    Box::pin(async move {
      if prop.name != "address-data" {
        return Err(FsError::NotImplemented);
      }
      let uid = match classify(path, &self.addressbook) {
        PathKind::Resource { uid } => uid,
        _ => return Err(FsError::NotFound),
      };
      let (_, vcard) = self.load_vcard(uid).await?;
      Ok(address_data_xml(&vcard).into_bytes())
    })
  }
}

// ─── card:address-data helpers ────────────────────────────────────────────────

fn address_data_xml(vcard: &str) -> String {
  format!(
    "<CARD:address-data \
     xmlns:CARD=\"urn:ietf:params:xml:ns:carddav\">{}</CARD:address-data>",
    escape_xml(vcard)
  )
}

fn address_data_prop(vcard: Option<&str>) -> DavProp {
  DavProp {
    name:      "address-data".to_string(),
    prefix:    Some("CARD".to_string()),
    namespace: Some("urn:ietf:params:xml:ns:carddav".to_string()),
    xml:       vcard.map(|v| address_data_xml(v).into_bytes()),
  }
}

fn escape_xml(s: &str) -> String {
  s.replace('&', "&amp;")
    .replace('<', "&lt;")
    .replace('>', "&gt;")
    .replace('"', "&quot;")
}
