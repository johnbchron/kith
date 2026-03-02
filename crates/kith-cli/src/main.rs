//! `kith` — terminal UI for the Kith contact store.
//!
//! # Usage
//!
//! ```
//! kith --url http://localhost:5232 --user alice --password secret
//! kith --config ~/.config/kith/config.toml
//! ```

mod app;
mod client;
mod ui;

use std::{io, time::Duration};

use anyhow::{Context, Result};
use app::App;
use clap::Parser;
use client::{ApiClient, ApiConfig};
use crossterm::{
  event::{self, Event},
  execute,
  terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use serde::Deserialize;

// ─── CLI args ─────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "kith", about = "Terminal UI for the Kith contact store")]
struct Args {
  /// Path to a TOML config file (url, username, password).
  #[arg(short, long, value_name = "FILE")]
  config: Option<std::path::PathBuf>,

  /// Base URL of the kith server (default: http://localhost:5232).
  #[arg(long, env = "KITH_URL")]
  url: Option<String>,

  /// API username.
  #[arg(long, env = "KITH_USER")]
  user: Option<String>,

  /// API password (plaintext).
  #[arg(long, env = "KITH_PASSWORD")]
  password: Option<String>,
}

// ─── Config file ──────────────────────────────────────────────────────────────

/// Shape of the optional TOML config file.
#[derive(Deserialize, Default)]
struct ConfigFile {
  #[serde(default)]
  url:      String,
  #[serde(default)]
  username: String,
  #[serde(default)]
  password: String,
}

// ─── Entry point ──────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
  let args = Args::parse();

  // Load config file if provided.
  let file_cfg: ConfigFile = if let Some(path) = &args.config {
    let raw = std::fs::read_to_string(path)
      .with_context(|| format!("reading config file {}", path.display()))?;
    toml::from_str(&raw).context("parsing config file")?
  } else {
    ConfigFile::default()
  };

  // CLI flags override config file, which overrides defaults.
  let api_config = ApiConfig {
    base_url: args
      .url
      .or_else(|| (!file_cfg.url.is_empty()).then(|| file_cfg.url.clone()))
      .unwrap_or_else(|| "http://localhost:5232".to_string()),
    username: args
      .user
      .or_else(|| (!file_cfg.username.is_empty()).then(|| file_cfg.username.clone()))
      .unwrap_or_default(),
    password: args
      .password
      .or_else(|| (!file_cfg.password.is_empty()).then(|| file_cfg.password.clone()))
      .unwrap_or_default(),
  };

  let client = ApiClient::new(api_config)?;
  let mut app = App::new(client);

  // Set up the terminal.
  enable_raw_mode().context("enabling raw mode")?;
  let mut stdout = io::stdout();
  execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;
  let backend = CrosstermBackend::new(stdout);
  let mut terminal = Terminal::new(backend).context("creating terminal")?;

  // Load initial data.
  let load_result = app.load_subjects().await;

  // Run the event loop; restore terminal even on error.
  let run_result = if load_result.is_ok() {
    run_event_loop(&mut terminal, &mut app).await
  } else {
    load_result
  };

  // Restore terminal regardless of result.
  disable_raw_mode().ok();
  execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
  terminal.show_cursor().ok();

  run_result
}

// ─── Event loop ───────────────────────────────────────────────────────────────

async fn run_event_loop(
  terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
  app: &mut App,
) -> Result<()> {
  // Ensure names are loaded for the initially-visible contacts.
  let ids: Vec<_> = app
    .subjects
    .iter()
    .take(50)
    .map(|s| s.subject_id)
    .collect();
  for id in ids {
    app.ensure_name(id).await;
  }

  loop {
    terminal.draw(|f| ui::draw(f, app)).context("drawing frame")?;

    // Poll for an event, yielding control to tokio while waiting.
    let maybe_event = tokio::task::block_in_place(|| {
      if event::poll(Duration::from_millis(50))? {
        Ok::<_, io::Error>(Some(event::read()?))
      } else {
        Ok(None)
      }
    })?;

    if let Some(evt) = maybe_event {
      match evt {
        Event::Key(key) => {
          let cont = app.handle_key(key).await?;
          if !cont {
            break;
          }
        }
        Event::Resize(_, _) => {
          // Terminal will redraw on next iteration.
        }
        _ => {}
      }
    }
  }

  Ok(())
}
