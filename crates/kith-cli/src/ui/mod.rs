//! TUI rendering — orchestrates all panes.

pub mod contact_detail;
pub mod contact_list;

use chrono::Local;
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Layout, Rect},
  style::Style,
  text::{Line, Span},
  widgets::{Block, Borders, Paragraph},
};

use crate::{
  app::{App, Screen},
  colors,
};

// ─── Root draw ────────────────────────────────────────────────────────────────

/// Main draw function called each frame.
pub fn draw(f: &mut Frame, app: &App) {
  let area = f.area();

  // Flood-fill every cell with the app background before anything else so
  // the native terminal colour never shows through transparent areas.
  f.render_widget(
    Block::default().style(Style::default().bg(colors::app_bg())),
    area,
  );

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

  let title = Span::styled(
    " kith",
    Style::default()
      .fg(colors::accent_text())
      .add_modifier(ratatui::style::Modifier::BOLD),
  );
  let hints = Span::styled(
    "  [/] search  [q] quit",
    colors::style_muted(),
  );
  let date_span = Span::styled(format!("{date} "), colors::style_subtle());

  let title_w = 5u16;
  let hints_w = 22u16;
  let date_w = date_span.content.len() as u16;
  let pad = area.width.saturating_sub(title_w + hints_w + date_w);

  let line = Line::from(vec![
    title,
    hints,
    Span::raw(" ".repeat(pad as usize)),
    date_span,
  ]);

  f.render_widget(
    Paragraph::new(line)
      .style(Style::default().bg(colors::accent_bg())),
    area,
  );
}

// ─── Body ─────────────────────────────────────────────────────────────────────

fn draw_body(f: &mut Frame, area: Rect, app: &App) {
  let cols = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
    .split(area);

  contact_list::draw(f, cols[0], app);

  if app.selected_subject_id.is_some() {
    contact_detail::draw(f, cols[1], app);
  } else {
    draw_empty_detail(f, cols[1]);
  }
}

fn draw_empty_detail(f: &mut Frame, area: Rect) {
  let block = Block::default()
    .title(Span::styled(" Detail ", colors::style_muted()))
    .borders(Borders::ALL)
    .border_style(colors::style_border())
    .style(Style::default().bg(colors::panel_bg()));
  let inner = block.inner(area);
  f.render_widget(block, area);
  f.render_widget(
    Paragraph::new(Span::styled(
      "Select a contact and press Enter.",
      colors::style_subtle(),
    )),
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
      "↑↓/jk scroll  Esc back  [/] prev/next contact  q quit",
    ),
  };

  let status = if app.status_msg.is_empty() {
    hints.to_string()
  } else {
    app.status_msg.clone()
  };

  let line = Line::from(vec![
    Span::styled(format!(" {mode_label} "), colors::style_mode_badge()),
    Span::styled(format!("  {status}"), colors::style_muted()),
  ]);

  f.render_widget(
    Paragraph::new(line).style(Style::default().bg(colors::accent_subtle())),
    area,
  );
}
