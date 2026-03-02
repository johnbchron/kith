//! Contact list pane — left panel.

use kith_core::subject::SubjectKind;
use ratatui::{
  Frame,
  layout::Rect,
  style::Style,
  text::{Line, Span},
  widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

use crate::{app::App, colors};

/// Render the contact list into `area`.
pub fn draw(f: &mut Frame, area: Rect, app: &App) {
  let filtered = app.filtered_subjects();
  let total = app.subjects.len();

  let title = if app.filter_active || !app.filter.is_empty() {
    format!(" Contacts ({}/{}) ", filtered.len(), total)
  } else {
    format!(" Contacts ({}) ", total)
  };

  let block = Block::default()
    .title(Span::styled(title, colors::style_muted()))
    .borders(Borders::ALL)
    .border_style(colors::style_border())
    .style(Style::default().bg(colors::panel_bg()));

  let inner_area = block.inner(area);
  f.render_widget(block, area);

  // Reserve the bottom row for the filter bar when active.
  let (list_area, filter_area) = if app.filter_active || !app.filter.is_empty() {
    (
      Rect { height: inner_area.height.saturating_sub(1), ..inner_area },
      Some(Rect {
        y:      inner_area.y + inner_area.height.saturating_sub(1),
        height: 1,
        ..inner_area
      }),
    )
  } else {
    (inner_area, None)
  };

  // Render the filter bar.
  if let Some(fa) = filter_area {
    let cursor = if app.filter_active { "_" } else { "" };
    let text = format!("/{}{}", app.filter, cursor);
    f.render_widget(
      Paragraph::new(Span::styled(text, Style::default().fg(colors::filter_prompt()))),
      fa,
    );
  }

  // Build list items — no per-item cursor style; ListState drives highlighting.
  let items: Vec<ListItem> = filtered
    .iter()
    .map(|subject| {
      let name = app
        .names
        .get(&subject.subject_id)
        .map(String::as_str)
        .unwrap_or("—");

      let icon = match subject.kind {
        SubjectKind::Person => "  ",
        SubjectKind::Organization => "  ",
        SubjectKind::Group => "  ",
      };

      ListItem::new(Line::from(vec![
        Span::styled(icon, colors::style_subtle()),
        Span::styled(name, colors::style_text()),
      ]))
    })
    .collect();

  let mut state = ListState::default();
  state.select(if filtered.is_empty() {
    None
  } else {
    Some(app.list_cursor)
  });

  f.render_stateful_widget(
    List::new(items)
      .highlight_style(colors::style_selected())
      .highlight_symbol("▶ "),
    list_area,
    &mut state,
  );
}
