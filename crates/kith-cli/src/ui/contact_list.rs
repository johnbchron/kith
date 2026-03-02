//! Contact list pane — left panel.

use kith_core::subject::SubjectKind;
use ratatui::{
  Frame,
  layout::Rect,
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::app::App;

/// Render the contact list into `area`.
pub fn draw(f: &mut Frame, area: Rect, app: &App) {
  let filtered = app.filtered_subjects();
  let total = app.subjects.len();

  // Title with count.
  let title = if app.filter_active || !app.filter.is_empty() {
    format!(" Contacts ({}/{}) ", filtered.len(), total)
  } else {
    format!(" Contacts ({}) ", total)
  };

  let block = Block::default()
    .title(title)
    .borders(Borders::ALL)
    .border_style(Style::default().fg(Color::DarkGray));

  // Build list items.
  let items: Vec<ListItem> = filtered
    .iter()
    .enumerate()
    .map(|(i, subject)| {
      let name = app
        .names
        .get(&subject.subject_id)
        .map(String::as_str)
        .unwrap_or("—");

      let kind_icon = match subject.kind {
        SubjectKind::Person => "👤 ",
        SubjectKind::Organization => "🏢 ",
        SubjectKind::Group => "👥 ",
      };

      let is_cursor = i == app.list_cursor;

      let style = if is_cursor {
        Style::default()
          .bg(Color::Blue)
          .fg(Color::White)
          .add_modifier(Modifier::BOLD)
      } else {
        Style::default()
      };

      ListItem::new(Line::from(vec![
        Span::styled(kind_icon, style),
        Span::styled(name.to_string(), style),
      ]))
    })
    .collect();

  // Build filter line if active.
  let mut inner_area = block.inner(area);
  f.render_widget(block, area);

  // If filter is active or set, show a filter bar at the bottom of the inner area.
  if app.filter_active || !app.filter.is_empty() && inner_area.height > 2 {
    let filter_area = Rect {
      x:      inner_area.x,
      y:      inner_area.y + inner_area.height - 1,
      width:  inner_area.width,
      height: 1,
    };
    inner_area.height = inner_area.height.saturating_sub(1);

    let filter_text = if app.filter_active {
      format!("/{}_", app.filter)
    } else {
      format!("/{}", app.filter)
    };
    let filter_style = Style::default().fg(Color::Yellow);
    f.render_widget(
      ratatui::widgets::Paragraph::new(filter_text).style(filter_style),
      filter_area,
    );
  }

  // Scrollable list with cursor tracking.
  let mut state = ListState::default();
  state.select(if filtered.is_empty() {
    None
  } else {
    Some(app.list_cursor)
  });

  f.render_stateful_widget(
    List::new(items)
      .highlight_style(
        Style::default()
          .bg(Color::Blue)
          .fg(Color::White)
          .add_modifier(Modifier::BOLD),
      )
      .highlight_symbol(""),
    inner_area,
    &mut state,
  );
}
