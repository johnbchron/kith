//! TUI rendering — orchestrates all panes.

pub mod contact_detail;
pub mod contact_list;

use chrono::Local;
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, Paragraph},
};

use crate::app::{App, Screen};

// ─── Root draw ────────────────────────────────────────────────────────────────

/// Main draw function called each frame.
pub fn draw(f: &mut Frame, app: &App) {
  let area = f.area();

  // Vertical stack: header, body, status bar.
  let rows = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
      Constraint::Length(1), // header
      Constraint::Min(0),    // body
      Constraint::Length(1), // status bar
    ])
    .split(area);

  draw_header(f, rows[0], app);
  draw_body(f, rows[1], app);
  draw_status(f, rows[2], app);
}

// ─── Header ───────────────────────────────────────────────────────────────────

fn draw_header(f: &mut Frame, area: Rect, _app: &App) {
  let date = Local::now().format("%Y-%m-%d").to_string();

  let left = Span::styled(
    " kith  [/] search  [q] quit",
    Style::default()
      .fg(Color::White)
      .add_modifier(Modifier::BOLD),
  );
  let right = Span::styled(
    format!("{date} "),
    Style::default().fg(Color::DarkGray),
  );

  // Simple left-right header: pad the middle.
  let left_width = left.content.len() as u16;
  let right_width = right.content.len() as u16;
  let pad = area
    .width
    .saturating_sub(left_width)
    .saturating_sub(right_width);

  let line = Line::from(vec![
    left,
    Span::raw(" ".repeat(pad as usize)),
    right,
  ]);

  let block = Block::default().style(Style::default().bg(Color::DarkGray));
  let inner = block.inner(area);
  f.render_widget(block, area);
  f.render_widget(Paragraph::new(line), inner);
}

// ─── Body ─────────────────────────────────────────────────────────────────────

fn draw_body(f: &mut Frame, area: Rect, app: &App) {
  // Split into left list pane (30%) and right detail pane (70%).
  let cols = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
    .split(area);

  // List pane: dimmed when detail is focused, normal otherwise.
  let list_style = if app.screen == Screen::ContactDetail {
    Style::default().fg(Color::DarkGray)
  } else {
    Style::default()
  };
  // Apply dim by overriding the block; contact_list::draw uses its own block.
  let _ = list_style; // contact_list handles its own highlighting
  contact_list::draw(f, cols[0], app);

  // Detail pane.
  if app.selected_subject_id.is_some() {
    contact_detail::draw(f, cols[1], app);
  } else {
    draw_empty_detail(f, cols[1]);
  }
}

fn draw_empty_detail(f: &mut Frame, area: Rect) {
  let block = Block::default()
    .title(" Detail ")
    .borders(Borders::ALL)
    .border_style(Style::default().fg(Color::DarkGray));
  let inner = block.inner(area);
  f.render_widget(block, area);
  f.render_widget(
    Paragraph::new(Line::from(vec![Span::styled(
      "Select a contact and press Enter.",
      Style::default().fg(Color::DarkGray),
    )])),
    inner,
  );
}

// ─── Status bar ───────────────────────────────────────────────────────────────

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
  let (mode_label, hints) = match &app.screen {
    Screen::ContactList if app.filter_active => (
      "SEARCH",
      "Type to filter  Esc cancel  Enter select",
    ),
    Screen::ContactList => (
      "NORMAL",
      "↑↓/jk navigate  / search  Enter detail  q quit",
    ),
    Screen::ContactDetail => (
      "DETAIL",
      "↑↓/jk scroll  [/] scroll  Esc back  [ ] prev  ] next  q quit",
    ),
  };

  let status = if app.status_msg.is_empty() {
    hints.to_string()
  } else {
    app.status_msg.clone()
  };

  let mode_span = Span::styled(
    format!(" {mode_label} "),
    Style::default()
      .fg(Color::Black)
      .bg(Color::Cyan)
      .add_modifier(Modifier::BOLD),
  );
  let hint_span = Span::styled(
    format!("  {status}"),
    Style::default().fg(Color::DarkGray),
  );

  let line = Line::from(vec![mode_span, hint_span]);
  f.render_widget(
    Paragraph::new(line).style(Style::default().bg(Color::Black)),
    area,
  );
}
