//! Contact detail pane — right panel, Facts tab.

use kith_core::fact::{ContactLabel, FactValue};
use ratatui::{
  Frame,
  layout::Rect,
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, Paragraph},
};

use crate::app::App;

// ─── Public entry ─────────────────────────────────────────────────────────────

/// Render the detail pane into `area`.
pub fn draw(f: &mut Frame, area: Rect, app: &App) {
  let subject_name = app
    .selected_subject_id
    .and_then(|id| app.names.get(&id))
    .map(String::as_str)
    .unwrap_or("(unknown)");

  let block = Block::default()
    .title(format!(" {subject_name} "))
    .borders(Borders::ALL)
    .border_style(Style::default().fg(Color::DarkGray));

  let inner = block.inner(area);
  f.render_widget(block, area);

  if app.selected_subject_id.is_none() {
    let hint = Paragraph::new("Press Enter to view a contact.")
      .style(Style::default().fg(Color::DarkGray));
    f.render_widget(hint, inner);
    return;
  }

  if app.facts.is_empty() {
    let empty = Paragraph::new("No active facts.")
      .style(Style::default().fg(Color::DarkGray));
    f.render_widget(empty, inner);
    return;
  }

  // Build lines: one per active fact, grouped visually.
  let mut lines: Vec<Line> = Vec::new();

  // Group by fact type discriminant for tidy display.
  let mut last_type = "";
  for rf in &app.facts {
    let type_str = rf.fact.value.discriminant();
    if type_str != last_type && !last_type.is_empty() {
      lines.push(Line::from(""));
    }
    last_type = type_str;

    let (type_label, value_str, extra) = format_fact(&rf.fact.value);
    let confidence_badge = match rf.fact.confidence {
      kith_core::fact::Confidence::Certain => "",
      kith_core::fact::Confidence::Probable => " ~",
      kith_core::fact::Confidence::Rumored => " ?",
    };

    let mut spans = vec![
      Span::styled(
        format!("{:<14}", type_label),
        Style::default()
          .fg(Color::Cyan)
          .add_modifier(Modifier::BOLD),
      ),
      Span::raw(value_str),
    ];

    if !extra.is_empty() {
      spans.push(Span::styled(
        format!("  {extra}"),
        Style::default().fg(Color::DarkGray),
      ));
    }

    if !confidence_badge.is_empty() {
      spans.push(Span::styled(
        confidence_badge.to_string(),
        Style::default().fg(Color::Yellow),
      ));
    }

    if !rf.fact.tags.is_empty() {
      spans.push(Span::styled(
        format!("  [{}]", rf.fact.tags.join(", ")),
        Style::default().fg(Color::DarkGray),
      ));
    }

    lines.push(Line::from(spans));
  }

  // Hints at the bottom (Phase A: read-only).
  lines.push(Line::from(""));
  lines.push(Line::from(vec![Span::styled(
    "[Phase B] history  [Phase C] edit / retract / add",
    Style::default().fg(Color::DarkGray),
  )]));

  let scroll_offset = app.detail_scroll as u16;
  let para = Paragraph::new(lines).scroll((scroll_offset, 0));
  f.render_widget(para, inner);
}

// ─── Fact formatting helpers ──────────────────────────────────────────────────

/// Returns `(type_label, value_string, extra_string)` for a fact value.
fn format_fact(value: &FactValue) -> (&'static str, String, String) {
  match value {
    FactValue::Name(n) => ("name", n.full.clone(), String::new()),
    FactValue::Alias(a) => (
      "alias",
      a.name.clone(),
      a.context.clone().unwrap_or_default(),
    ),
    FactValue::Photo(_) => ("photo", "(photo)".into(), String::new()),
    FactValue::Birthday(d) => ("birthday", d.format("%Y-%m-%d").to_string(), String::new()),
    FactValue::Anniversary(d) => ("anniversary", d.format("%Y-%m-%d").to_string(), String::new()),
    FactValue::Gender(g) => ("gender", g.clone(), String::new()),

    FactValue::Email(e) => ("email", e.address.clone(), format_label(&e.label)),
    FactValue::Phone(p) => ("phone", p.number.clone(), format_label(&p.label)),
    FactValue::Address(a) => {
      let parts: Vec<&str> = [
        a.street.as_deref(),
        a.locality.as_deref(),
        a.region.as_deref(),
        a.country.as_deref(),
      ]
      .into_iter()
      .flatten()
      .collect();
      ("address", parts.join(", "), format_label(&a.label))
    }
    FactValue::Url(u) => ("url", u.url.clone(), String::new()),
    FactValue::Im(i) => ("im", i.handle.clone(), i.service.clone()),
    FactValue::Social(s) => ("social", s.handle.clone(), s.platform.clone()),

    FactValue::Relationship(r) => {
      let other = r.other_name.as_deref().unwrap_or("(in store)");
      ("relation", other.to_string(), r.relation.clone())
    }
    FactValue::OrgMembership(o) => {
      let extra = o
        .title
        .as_deref()
        .unwrap_or(o.role.as_deref().unwrap_or(""))
        .to_string();
      ("org", o.org_name.clone(), extra)
    }
    FactValue::GroupMembership(g) => ("group", g.group_name.clone(), String::new()),

    FactValue::Note(n) => ("note", n.clone(), String::new()),
    FactValue::Meeting(m) => (
      "meeting",
      m.summary.clone(),
      m.location.clone().unwrap_or_default(),
    ),
    FactValue::Introduction(i) => ("intro", i.clone(), String::new()),
    FactValue::Custom { key, value } => ("custom", value.to_string(), key.clone()),
  }
}

fn format_label(label: &ContactLabel) -> String {
  match label {
    ContactLabel::Work => "work".into(),
    ContactLabel::Home => "home".into(),
    ContactLabel::Other => "other".into(),
    ContactLabel::Custom(s) => s.clone(),
  }
}
