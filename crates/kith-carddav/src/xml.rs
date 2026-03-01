//! WebDAV / CardDAV XML parsing and generation.
//!
//! Uses `quick-xml`'s writer API for generation and a hand-written parser
//! for reading PROPFIND request bodies.

use std::io::Cursor;

use quick_xml::{
  Writer,
  events::{BytesEnd, BytesStart, BytesText, Event},
};

use crate::error::Error;

// ─── Namespaces
// ───────────────────────────────────────────────────────────────

pub const NS_DAV: &str = "DAV:";
pub const NS_CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";

// ─── PROPFIND request ────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum PropfindRequest {
  AllProp,
  PropNames,
  Prop(Vec<PropName>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropName {
  ResourceType,
  DisplayName,
  GetContentType,
  GetETag,
  GetContentLength,
  GetLastModified,
  CurrentUserPrincipal,
  AddressbookHomeSet,
  AddressbookDescription,
  SupportedAddressData,
  AddressData,
  Unknown(String),
}

/// Parse a PROPFIND request body. Empty/missing body → `AllProp`.
pub fn parse_propfind(xml: &[u8]) -> Result<PropfindRequest, Error> {
  if xml.is_empty() {
    return Ok(PropfindRequest::AllProp);
  }

  let mut reader = quick_xml::Reader::from_reader(xml);
  reader.config_mut().trim_text(true);

  let mut in_prop = false;
  let mut names: Vec<PropName> = Vec::new();
  let mut buf = Vec::new();

  loop {
    match reader.read_event_into(&mut buf) {
      Ok(Event::Start(ref e) | Event::Empty(ref e)) => {
        let name_buf = e.name();
        let local = local_name(name_buf.as_ref());
        match local {
          b"allprop" => return Ok(PropfindRequest::AllProp),
          b"propname" => return Ok(PropfindRequest::PropNames),
          b"prop" => {
            in_prop = true;
          }
          _ if in_prop => {
            names.push(parse_prop_name(local));
          }
          _ => {}
        }
      }
      Ok(Event::End(ref e)) => {
        let end_name = e.name();
        if local_name(end_name.as_ref()) == b"prop" {
          in_prop = false;
        }
      }
      Ok(Event::Eof) => break,
      Err(e) => return Err(Error::Xml(e.to_string())),
      _ => {}
    }
    buf.clear();
  }

  if names.is_empty() {
    Ok(PropfindRequest::AllProp)
  } else {
    Ok(PropfindRequest::Prop(names))
  }
}

fn local_name(name: &[u8]) -> &[u8] {
  // strip "prefix:" if present
  if let Some(pos) = name.iter().rposition(|&b| b == b':') {
    &name[pos + 1..]
  } else {
    name
  }
}

fn parse_prop_name(local: &[u8]) -> PropName {
  match local {
    b"resourcetype" => PropName::ResourceType,
    b"displayname" => PropName::DisplayName,
    b"getcontenttype" => PropName::GetContentType,
    b"getetag" => PropName::GetETag,
    b"getcontentlength" => PropName::GetContentLength,
    b"getlastmodified" => PropName::GetLastModified,
    b"current-user-principal" => PropName::CurrentUserPrincipal,
    b"addressbook-home-set" => PropName::AddressbookHomeSet,
    b"addressbook-description" => PropName::AddressbookDescription,
    b"supported-address-data" => PropName::SupportedAddressData,
    b"address-data" => PropName::AddressData,
    other => PropName::Unknown(String::from_utf8_lossy(other).into_owned()),
  }
}

// ─── PROPFIND response ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum ResourceType {
  Principal,
  Collection,
  Addressbook,
}

#[derive(Debug, Clone)]
pub enum Property {
  ResourceType(Vec<ResourceType>),
  DisplayName(String),
  GetContentType(String),
  GetETag(String),
  GetContentLength(u64),
  GetLastModified(String),
  CurrentUserPrincipal(String),
  AddressbookHomeSet(String),
  AddressbookDescription(String),
  SupportedAddressData,
  AddressData(String),
}

pub struct MultistatusBuilder {
  writer: Writer<Cursor<Vec<u8>>>,
}

impl Default for MultistatusBuilder {
  fn default() -> Self { Self::new() }
}

impl MultistatusBuilder {
  pub fn new() -> Self {
    let cursor = Cursor::new(Vec::new());
    let mut writer = Writer::new(cursor);

    // XML declaration
    writer
      .write_event(Event::Decl(quick_xml::events::BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        None,
      )))
      .unwrap();

    // <D:multistatus>
    let mut ms = BytesStart::new("D:multistatus");
    ms.push_attribute(("xmlns:D", NS_DAV));
    ms.push_attribute(("xmlns:card", NS_CARDDAV));
    writer.write_event(Event::Start(ms)).unwrap();

    Self { writer }
  }

  pub fn response(&mut self, href: &str) -> ResponseBuilder<'_> {
    ResponseBuilder {
      parent: self,
      href:   href.to_string(),
    }
  }

  pub fn finish(mut self) -> Vec<u8> {
    self
      .writer
      .write_event(Event::End(BytesEnd::new("D:multistatus")))
      .unwrap();
    self.writer.into_inner().into_inner()
  }
}

pub struct ResponseBuilder<'a> {
  parent: &'a mut MultistatusBuilder,
  href:   String,
}

impl<'a> ResponseBuilder<'a> {
  pub fn propstat_ok(self, props: &[Property]) -> &'a mut MultistatusBuilder {
    let w = &mut self.parent.writer;

    write_start(w, "D:response");
    write_text_elem(w, "D:href", &self.href);
    write_start(w, "D:propstat");
    write_start(w, "D:prop");

    for prop in props {
      write_property(w, prop);
    }

    write_end(w, "D:prop");
    write_text_elem(w, "D:status", "HTTP/1.1 200 OK");
    write_end(w, "D:propstat");
    write_end(w, "D:response");

    self.parent
  }

  /// Emit a bare `<D:status>` response (no propstat), used for 404 in
  /// addressbook-multiget when the resource doesn't exist at all.
  pub fn status_not_found(self) -> &'a mut MultistatusBuilder {
    let w = &mut self.parent.writer;
    write_start(w, "D:response");
    write_text_elem(w, "D:href", &self.href);
    write_text_elem(w, "D:status", "HTTP/1.1 404 Not Found");
    write_end(w, "D:response");
    self.parent
  }

  pub fn propstat_not_found(
    self,
    names: &[PropName],
  ) -> &'a mut MultistatusBuilder {
    let w = &mut self.parent.writer;

    write_start(w, "D:response");
    write_text_elem(w, "D:href", &self.href);
    write_start(w, "D:propstat");
    write_start(w, "D:prop");

    for name in names {
      write_prop_name_elem(w, name);
    }

    write_end(w, "D:prop");
    write_text_elem(w, "D:status", "HTTP/1.1 404 Not Found");
    write_end(w, "D:propstat");
    write_end(w, "D:response");

    self.parent
  }
}

// ─── XML writer helpers
// ───────────────────────────────────────────────────────

fn write_start(w: &mut Writer<Cursor<Vec<u8>>>, tag: &str) {
  w.write_event(Event::Start(BytesStart::new(tag))).unwrap();
}

fn write_end(w: &mut Writer<Cursor<Vec<u8>>>, tag: &str) {
  w.write_event(Event::End(BytesEnd::new(tag))).unwrap();
}

fn write_text_elem(w: &mut Writer<Cursor<Vec<u8>>>, tag: &str, text: &str) {
  write_start(w, tag);
  w.write_event(Event::Text(BytesText::new(text))).unwrap();
  write_end(w, tag);
}

fn write_empty(w: &mut Writer<Cursor<Vec<u8>>>, tag: &str) {
  w.write_event(Event::Empty(BytesStart::new(tag))).unwrap();
}

fn write_empty_with_attr(
  w: &mut Writer<Cursor<Vec<u8>>>,
  tag: &str,
  attrs: &[(&str, &str)],
) {
  let mut el = BytesStart::new(tag);
  for (k, v) in attrs {
    el.push_attribute((*k, *v));
  }
  w.write_event(Event::Empty(el)).unwrap();
}

fn write_href_element(
  w: &mut Writer<Cursor<Vec<u8>>>,
  wrapper: &str,
  href: &str,
) {
  write_start(w, wrapper);
  write_text_elem(w, "D:href", href);
  write_end(w, wrapper);
}

fn write_property(w: &mut Writer<Cursor<Vec<u8>>>, prop: &Property) {
  match prop {
    Property::ResourceType(types) => {
      write_start(w, "D:resourcetype");
      for rt in types {
        match rt {
          ResourceType::Collection => write_empty(w, "D:collection"),
          ResourceType::Addressbook => write_empty(w, "card:addressbook"),
          ResourceType::Principal => write_empty(w, "D:principal"),
        }
      }
      write_end(w, "D:resourcetype");
    }
    Property::DisplayName(name) => write_text_elem(w, "D:displayname", name),
    Property::GetContentType(ct) => write_text_elem(w, "D:getcontenttype", ct),
    Property::GetETag(etag) => write_text_elem(w, "D:getetag", etag),
    Property::GetContentLength(len) => {
      write_text_elem(w, "D:getcontentlength", &len.to_string())
    }
    Property::GetLastModified(dt) => {
      write_text_elem(w, "D:getlastmodified", dt)
    }
    Property::CurrentUserPrincipal(href) => {
      write_href_element(w, "D:current-user-principal", href)
    }
    Property::AddressbookHomeSet(href) => {
      write_href_element(w, "card:addressbook-home-set", href)
    }
    Property::AddressbookDescription(desc) => {
      write_text_elem(w, "card:addressbook-description", desc)
    }
    Property::SupportedAddressData => {
      write_start(w, "card:supported-address-data");
      write_empty_with_attr(w, "card:address-data-type", &[
        ("content-type", "text/vcard"),
        ("version", "3.0"),
      ]);
      write_empty_with_attr(w, "card:address-data-type", &[
        ("content-type", "text/vcard"),
        ("version", "4.0"),
      ]);
      write_end(w, "card:supported-address-data");
    }
    Property::AddressData(data) => write_text_elem(w, "card:address-data", data),
  }
}

fn write_prop_name_elem(w: &mut Writer<Cursor<Vec<u8>>>, name: &PropName) {
  let tag = match name {
    PropName::ResourceType => "D:resourcetype",
    PropName::DisplayName => "D:displayname",
    PropName::GetContentType => "D:getcontenttype",
    PropName::GetETag => "D:getetag",
    PropName::GetContentLength => "D:getcontentlength",
    PropName::GetLastModified => "D:getlastmodified",
    PropName::CurrentUserPrincipal => "D:current-user-principal",
    PropName::AddressbookHomeSet => "card:addressbook-home-set",
    PropName::AddressbookDescription => "card:addressbook-description",
    PropName::SupportedAddressData => "card:supported-address-data",
    PropName::AddressData => "card:address-data",
    PropName::Unknown(s) => s.as_str(),
  };
  write_empty(w, tag);
}

// ─── REPORT request parsing ──────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum ReportKind {
  /// `card:addressbook-multiget` — fetch specific resources by href.
  Multiget,
  /// `card:addressbook-query` — fetch resources matching a filter.
  Query,
}

#[derive(Debug)]
pub struct ReportRequest {
  pub kind:  ReportKind,
  /// The `<D:prop>` names requested by the client.
  pub props: Vec<PropName>,
  /// Hrefs listed by the client (only populated for `Multiget`).
  pub hrefs: Vec<String>,
}

/// Parse an `addressbook-multiget` or `addressbook-query` request body.
pub fn parse_report(xml: &[u8]) -> Result<ReportRequest, Error> {
  if xml.is_empty() {
    return Err(Error::BadRequest("empty REPORT body".into()));
  }

  let mut reader = quick_xml::Reader::from_reader(xml);
  reader.config_mut().trim_text(true);

  let mut kind: Option<ReportKind> = None;
  let mut props: Vec<PropName> = Vec::new();
  let mut hrefs: Vec<String> = Vec::new();
  let mut in_prop = false;
  let mut in_href = false;
  let mut buf = Vec::new();

  loop {
    match reader.read_event_into(&mut buf) {
      Ok(Event::Start(ref e) | Event::Empty(ref e)) => {
        let name_buf = e.name();
        let local = local_name(name_buf.as_ref());
        match local {
          b"addressbook-multiget" => {
            kind = Some(ReportKind::Multiget);
          }
          b"addressbook-query" => {
            kind = Some(ReportKind::Query);
          }
          b"prop" => {
            in_prop = true;
          }
          b"href" if !in_prop => {
            in_href = true;
          }
          _ if in_prop => {
            props.push(parse_prop_name(local));
          }
          _ => {}
        }
      }
      Ok(Event::Text(ref e)) => {
        if in_href {
          hrefs.push(e.unescape().unwrap_or_default().into_owned());
          in_href = false;
        }
      }
      Ok(Event::End(ref e)) => {
        let name_buf = e.name();
        let local = local_name(name_buf.as_ref());
        if local == b"prop" {
          in_prop = false;
        }
        if local == b"href" {
          in_href = false;
        }
      }
      Ok(Event::Eof) => break,
      Err(e) => return Err(Error::Xml(e.to_string())),
      _ => {}
    }
    buf.clear();
  }

  let kind = kind.ok_or_else(|| {
    Error::BadRequest("REPORT body is not a recognized CardDAV report".into())
  })?;

  // If no props were explicitly requested, treat as allprop.
  if props.is_empty() {
    props = vec![PropName::GetETag, PropName::AddressData];
  }

  Ok(ReportRequest { kind, props, hrefs })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parse_allprop() {
    let xml = br#"<?xml version="1.0"?>
    <D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#;
    assert_eq!(parse_propfind(xml).unwrap(), PropfindRequest::AllProp);
  }

  #[test]
  fn parse_prop_list() {
    let xml = br#"<?xml version="1.0"?>
    <D:propfind xmlns:D="DAV:">
      <D:prop>
        <D:getetag/>
        <D:getcontenttype/>
        <D:displayname/>
      </D:prop>
    </D:propfind>"#;
    let result = parse_propfind(xml).unwrap();
    assert_eq!(
      result,
      PropfindRequest::Prop(vec![
        PropName::GetETag,
        PropName::GetContentType,
        PropName::DisplayName,
      ])
    );
  }

  #[test]
  fn empty_body_is_allprop() {
    assert_eq!(parse_propfind(b"").unwrap(), PropfindRequest::AllProp);
  }

  #[test]
  fn multistatus_round_trip() {
    let mut ms = MultistatusBuilder::new();
    ms.response("/dav/addressbooks/personal/")
      .propstat_ok(&[Property::DisplayName("Personal".to_string())]);
    ms.response("/dav/addressbooks/personal/abc.vcf")
      .propstat_ok(&[Property::GetETag("\"abc123\"".to_string())]);

    let bytes = ms.finish();
    let xml_str = std::str::from_utf8(&bytes).unwrap();

    // Verify both hrefs appear
    assert!(
      xml_str.contains("/dav/addressbooks/personal/"),
      "missing collection href"
    );
    assert!(
      xml_str.contains("/dav/addressbooks/personal/abc.vcf"),
      "missing resource href"
    );
    assert!(xml_str.contains("200 OK"), "missing 200 status");
    assert!(xml_str.contains("abc123"), "missing etag");
    assert!(xml_str.contains("Personal"), "missing display name");
  }
}
