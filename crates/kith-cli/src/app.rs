//! Application state machine and event dispatcher.

use std::{collections::HashMap, sync::Arc};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use kith_core::{lifecycle::ResolvedFact, subject::Subject};
use uuid::Uuid;

use crate::client::ApiClient;

// ─── Screen ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
  /// Focus on the contact list; right pane is empty or shows a preview.
  ContactList,
  /// Focus on the contact detail pane.
  ContactDetail,
}

// ─── App ──────────────────────────────────────────────────────────────────────

/// Top-level application state.
pub struct App {
  /// Current screen / keyboard focus.
  pub screen: Screen,

  /// All subjects returned by the API on startup.
  pub subjects: Vec<Subject>,

  /// Cached display names per subject. Populated lazily on first detail view.
  pub names: HashMap<Uuid, String>,

  /// Current fuzzy-filter string (only active when `filter_active`).
  pub filter: String,

  /// Whether the user is typing a filter query.
  pub filter_active: bool,

  /// Cursor position within the *filtered* subject list.
  pub list_cursor: usize,

  /// Scroll offset within the detail facts list.
  pub detail_scroll: usize,

  /// UUID of the currently-selected subject (detail pane).
  pub selected_subject_id: Option<Uuid>,

  /// Active facts for the currently-selected subject.
  pub facts: Vec<ResolvedFact>,

  /// One-line status message shown in the status bar.
  pub status_msg: String,

  /// Shared HTTP client.
  pub client: Arc<ApiClient>,
}

impl App {
  /// Create an [`App`] with an empty subject list.
  pub fn new(client: ApiClient) -> Self {
    Self {
      screen: Screen::ContactList,
      subjects: Vec::new(),
      names: HashMap::new(),
      filter: String::new(),
      filter_active: false,
      list_cursor: 0,
      detail_scroll: 0,
      selected_subject_id: None,
      facts: Vec::new(),
      status_msg: String::new(),
      client: Arc::new(client),
    }
  }

  // ── Data loading ──────────────────────────────────────────────────────────

  /// Fetch all subjects from the API and populate `self.subjects`.
  pub async fn load_subjects(&mut self) -> anyhow::Result<()> {
    self.status_msg = "Loading contacts…".into();
    match self.client.list_subjects().await {
      Ok(subjects) => {
        self.subjects = subjects;
        self.list_cursor = 0;
        self.status_msg = String::new();
        Ok(())
      }
      Err(e) => {
        self.status_msg = format!("Error: {e}");
        Err(e)
      }
    }
  }

  /// Load the display name for `subject_id` if not already cached.
  pub async fn ensure_name(&mut self, subject_id: Uuid) {
    if self.names.contains_key(&subject_id) {
      return;
    }
    if let Ok(facts) = self.client.get_name_facts(subject_id).await {
      let name = facts.iter().find_map(|rf| {
        if !rf.status.is_active() {
          return None;
        }
        if let kith_core::fact::FactValue::Name(n) = &rf.fact.value {
          Some(n.full.clone())
        } else {
          None
        }
      });
      if let Some(n) = name {
        self.names.insert(subject_id, n);
      }
    }
  }

  /// Load all facts for `subject_id` into `self.facts`.
  async fn load_facts(&mut self, subject_id: Uuid) -> anyhow::Result<()> {
    self.status_msg = "Loading…".into();
    match self.client.get_facts(subject_id, false).await {
      Ok(facts) => {
        self.facts = facts;
        self.detail_scroll = 0;
        self.status_msg = String::new();
        Ok(())
      }
      Err(e) => {
        self.status_msg = format!("Error: {e}");
        Err(e)
      }
    }
  }

  // ── Filtered list ─────────────────────────────────────────────────────────

  /// Returns subjects that match the current filter query.
  pub fn filtered_subjects(&self) -> Vec<&Subject> {
    if self.filter.is_empty() {
      return self.subjects.iter().collect();
    }
    let matcher = SkimMatcherV2::default();
    self
      .subjects
      .iter()
      .filter(|s| {
        let name = self
          .names
          .get(&s.subject_id)
          .map(String::as_str)
          .unwrap_or_default();
        matcher.fuzzy_match(name, &self.filter).is_some()
          || matcher
            .fuzzy_match(&s.subject_id.to_string(), &self.filter)
            .is_some()
      })
      .collect()
  }

  /// The subject under the list cursor in the filtered view, if any.
  pub fn cursor_subject(&self) -> Option<&Subject> {
    let list = self.filtered_subjects();
    list.get(self.list_cursor).copied()
  }

  // ── Key handling ──────────────────────────────────────────────────────────

  /// Process a key event. Returns `true` to continue, `false` to quit.
  pub async fn handle_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
    // Global: Ctrl-C quits from anywhere.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
      return Ok(false);
    }

    // Filter input mode: all printable keys go into the filter string.
    if self.filter_active {
      return self.handle_filter_key(key).await;
    }

    match self.screen {
      Screen::ContactList => self.handle_list_key(key).await,
      Screen::ContactDetail => self.handle_detail_key(key).await,
    }
  }

  async fn handle_filter_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
    match key.code {
      KeyCode::Esc => {
        self.filter_active = false;
        self.filter.clear();
        self.list_cursor = 0;
      }
      KeyCode::Enter => {
        self.filter_active = false;
        self.list_cursor = 0;
        // Immediately open detail if there's exactly one match.
        let list = self.filtered_subjects();
        if list.len() == 1 {
          let id = list[0].subject_id;
          drop(list);
          self.open_detail(id).await?;
        }
      }
      KeyCode::Backspace => {
        self.filter.pop();
        self.list_cursor = 0;
      }
      KeyCode::Char(c) => {
        self.filter.push(c);
        self.list_cursor = 0;
      }
      _ => {}
    }
    Ok(true)
  }

  async fn handle_list_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
    match key.code {
      // Quit
      KeyCode::Char('q') => return Ok(false),

      // Navigation
      KeyCode::Down | KeyCode::Char('j') => {
        let len = self.filtered_subjects().len();
        if len > 0 && self.list_cursor + 1 < len {
          self.list_cursor += 1;
          // Lazily load name for the newly-visible contact.
          if let Some(id) = self.cursor_subject().map(|s| s.subject_id) {
            self.ensure_name(id).await;
          }
        }
      }
      KeyCode::Up | KeyCode::Char('k') => {
        if self.list_cursor > 0 {
          self.list_cursor -= 1;
          if let Some(id) = self.cursor_subject().map(|s| s.subject_id) {
            self.ensure_name(id).await;
          }
        }
      }

      // Open detail
      KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
        if let Some(id) = self.cursor_subject().map(|s| s.subject_id) {
          self.open_detail(id).await?;
        }
      }

      // Filter
      KeyCode::Char('/') => {
        self.filter_active = true;
        self.filter.clear();
        self.list_cursor = 0;
      }

      _ => {}
    }
    Ok(true)
  }

  async fn handle_detail_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
    match key.code {
      // Quit
      KeyCode::Char('q') => return Ok(false),

      // Back to list
      KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
        self.screen = Screen::ContactList;
        self.selected_subject_id = None;
        self.facts.clear();
      }

      // Scroll detail
      KeyCode::Down | KeyCode::Char('j') => {
        if self.detail_scroll + 1 < self.facts.len() {
          self.detail_scroll += 1;
        }
      }
      KeyCode::Up | KeyCode::Char('k') => {
        if self.detail_scroll > 0 {
          self.detail_scroll -= 1;
        }
      }

      // Navigate list from detail (for quick switching)
      KeyCode::Char(']') | KeyCode::PageDown => {
        let len = self.filtered_subjects().len();
        if len > 0 && self.list_cursor + 1 < len {
          self.list_cursor += 1;
          if let Some(id) = self.cursor_subject().map(|s| s.subject_id) {
            self.open_detail(id).await?;
          }
        }
      }
      KeyCode::Char('[') | KeyCode::PageUp => {
        if self.list_cursor > 0 {
          self.list_cursor -= 1;
          if let Some(id) = self.cursor_subject().map(|s| s.subject_id) {
            self.open_detail(id).await?;
          }
        }
      }

      _ => {}
    }
    Ok(true)
  }

  /// Transition to `ContactDetail` for `subject_id`, loading facts.
  async fn open_detail(&mut self, subject_id: Uuid) -> anyhow::Result<()> {
    self.ensure_name(subject_id).await;
    self.load_facts(subject_id).await?;
    self.selected_subject_id = Some(subject_id);
    self.screen = Screen::ContactDetail;
    Ok(())
  }
}
