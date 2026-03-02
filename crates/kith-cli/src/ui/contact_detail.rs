//! Contact detail pane — right panel, Facts tab.

use kith_core::fact::{Confidence, ContactLabel, FactValue};
use ratatui::{
  Frame,
  layout::Rect,
  style::Style,
  text::{Line, Span},
  widgets::{Block, Borders, Paragraph},
};

use crate::{app::App, colors};

// ─── Public entry ─────────────────────────────────────────────────────────────

/// Render the detail pane into `area`.
pub fn draw(f: &mut Frame, area: Rect, app: &App) {
  let subject_name = app
    .selected_subject_id
    .and_then(|id| app.names.get(&id))
    .map(String::as_str)
    .unwrap_or("(unknown)");

  let block = Block::default()
    .title(Span::styled(
      format!(" {subject_name} "),
      colors::style_accent_hi(),
    ))
    .borders(Borders::ALL)
    .border_style(colors::style_border_focus())
    .style(Style::default().bg(colors::panel_bg()));

  let inner = block.inner(area);
  f.render_widget(block, area);

  if app.selected_subject_id.is_none() {
    f.render_widget(
      Paragraph::new(Span::styled(
        "Select a contact and press Enter.",
        colors::style_muted(),
      )),
      inner,
    );
    return;
  }

  if app.facts.is_empty() {
    f.render_widget(
      Paragraph::new(Span::styled(
        "No active facts for this contact.",
        colors::style_muted(),
      )),
      inner,
    );
    return;
  }

  // Build one line per active fact, with a blank line between type groups.
  let mut lines: Vec<Line> = Vec::new();
  let mut last_type = "";

  for rf in &app.facts {
    let type_str = rf.fact.value.discriminant();

    if type_str != last_type && !last_type.is_empty() {
      lines.push(Line::from(""));
    }
    last_type = type_str;

    let (type_label, value_str, extra) = format_fact(&rf.fact.value);

    // Confidence suffix.
    let conf_span: Option<Span> = match rf.fact.confidence {
      Confidence::Certain => None,
      Confidence::Probable => Some(Span::styled(
        "  ~probable",
        Style::default().fg(colors::probable()),
      )),
      Confidence::Rumored => Some(Span::styled(
        "  ?rumored",
        Style::default().fg(colors::rumored()),
      )),
    };

    let mut spans: Vec<Span> = vec![
      Span::styled(format!("{:<14}", type_label), colors::style_accent_text()),
      Span::styled(value_str, colors::style_text()),
    ];

    if !extra.is_empty() {
      spans.push(Span::styled(
        format!("  {extra}"),
        colors::style_muted(),
      ));
    }

    if let Some(cs) = conf_span {
      spans.push(cs);
    }

    if !rf.fact.tags.is_empty() {
      spans.push(Span::styled(
        format!("  [{}]", rf.fact.tags.join(", ")),
        colors::style_subtle(),
      ));
    }

    lines.push(Line::from(spans));
  }

  // Phase-B/C hint footer.
  lines.push(Line::from(""));
  lines.push(Line::from(vec![Span::styled(
    "Phase B: history  Phase C: edit / retract / add",
    colors::style_subtle(),
  )]));

  let scroll = app.detail_scroll as u16;
  f.render_widget(
    Paragraph::new(lines).scroll((scroll, 0)),
    inner,
  );
}

// ─── Fact formatting ──────────────────────────────────────────────────────────

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
    FactValue::Anniversary(d) => (
      "anniversary",
      d.format("%Y-%m-%d").to_string(),
      String::new(),
    ),
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
        .or(o.role.as_deref())
        .unwrap_or("")
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
