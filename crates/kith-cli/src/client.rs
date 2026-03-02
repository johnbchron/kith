//! Async HTTP client wrapping the kith JSON API.

use anyhow::{Context, Result, anyhow};
use kith_core::{lifecycle::ResolvedFact, subject::Subject};
use reqwest::Client;
use std::time::Duration;
use uuid::Uuid;

/// Connection settings for the kith API.
#[derive(Debug, Clone)]
pub struct ApiConfig {
  pub base_url: String,
  pub username: String,
  pub password: String,
}

/// Async HTTP client for the kith JSON REST API.
///
/// Cheap to clone — the inner [`reqwest::Client`] is `Arc`-based.
#[derive(Clone)]
pub struct ApiClient {
  client: Client,
  config: ApiConfig,
}

impl ApiClient {
  pub fn new(config: ApiConfig) -> Result<Self> {
    let client = Client::builder()
      .timeout(Duration::from_secs(30))
      .build()
      .context("failed to build HTTP client")?;
    Ok(Self { client, config })
  }

  fn url(&self, path: &str) -> String {
    format!(
      "{}/api{}",
      self.config.base_url.trim_end_matches('/'),
      path
    )
  }

  fn auth<'a>(&'a self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    if self.config.username.is_empty() {
      req
    } else {
      req.basic_auth(&self.config.username, Some(&self.config.password))
    }
  }

  // ── Subjects ──────────────────────────────────────────────────────────────

  /// `GET /api/subjects`
  pub async fn list_subjects(&self) -> Result<Vec<Subject>> {
    let resp = self
      .auth(self.client.get(self.url("/subjects")))
      .send()
      .await
      .context("GET /subjects failed")?;

    if !resp.status().is_success() {
      return Err(anyhow!("GET /subjects → {}", resp.status()));
    }
    resp.json().await.context("deserialising subjects")
  }

  // ── Facts ─────────────────────────────────────────────────────────────────

  /// `GET /api/facts?subject_id=<id>[&fact_type=<t>]`
  pub async fn get_facts(
    &self,
    subject_id: Uuid,
    include_inactive: bool,
  ) -> Result<Vec<ResolvedFact>> {
    let resp = self
      .auth(self.client.get(self.url("/facts")))
      .query(&[
        ("subject_id", subject_id.to_string()),
        ("include_inactive", include_inactive.to_string()),
      ])
      .send()
      .await
      .context("GET /facts failed")?;

    if !resp.status().is_success() {
      return Err(anyhow!("GET /facts → {}", resp.status()));
    }
    resp.json().await.context("deserialising facts")
  }

  /// `GET /api/facts?subject_id=<id>&fact_type=name`
  pub async fn get_name_facts(&self, subject_id: Uuid) -> Result<Vec<ResolvedFact>> {
    let resp = self
      .auth(self.client.get(self.url("/facts")))
      .query(&[
        ("subject_id", subject_id.to_string()),
        ("fact_type", "name".to_string()),
      ])
      .send()
      .await
      .context("GET /facts?fact_type=name failed")?;

    if !resp.status().is_success() {
      return Err(anyhow!("GET /facts (name) → {}", resp.status()));
    }
    resp.json().await.context("deserialising name facts")
  }
}
