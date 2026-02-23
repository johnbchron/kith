//! kith-carddav server binary.
//!
//! Reads `config.toml` (or the path specified with `--config`), opens an
//! in-process SQLite store, and serves CardDAV over HTTP.
//!
//! # Password hash generation
//!
//! To generate the argon2 PHC string for `auth_password_hash` in config.toml:
//!
//! ```
//! cargo run -p kith-carddav --bin server -- --hash-password
//! ```

use std::{
  path::{Path, PathBuf},
  sync::Arc,
};

use anyhow::Context as _;
use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
use clap::Parser;
use kith_carddav::{AppState, ServerConfig, auth::AuthConfig};
use kith_store_sqlite::SqliteStore;
use rand_core::OsRng;
use tokio::net::TcpListener;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(author, version, about = "Kith CardDAV server")]
struct Cli {
  /// Path to the TOML configuration file.
  #[arg(short, long, default_value = "config.toml")]
  config: PathBuf,

  /// Print the argon2 hash for a password entered on stdin and exit.
  #[arg(long)]
  hash_password: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  // Initialise tracing.
  tracing_subscriber::fmt()
    .with_env_filter(
      EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy(),
    )
    .init();

  let cli = Cli::parse();

  // Helper mode: hash a password and exit.
  if cli.hash_password {
    let password = rpassword_or_stdin()?;
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
      .hash_password(password.as_bytes(), &salt)
      .map_err(|e| anyhow::anyhow!("argon2 error: {e}"))?
      .to_string();
    println!("{hash}");
    return Ok(());
  }

  // Load configuration.
  let settings = config::Config::builder()
    .add_source(config::File::from(cli.config).required(false))
    .add_source(config::Environment::with_prefix("KITH"))
    .build()
    .context("failed to read config file")?;

  let server_cfg: ServerConfig = settings
    .try_deserialize()
    .context("failed to deserialise ServerConfig")?;

  // Expand `~` in store path.
  let store_path = expand_tilde(&server_cfg.store_path);

  // Open SQLite store.
  let store = SqliteStore::open(&store_path)
    .await
    .with_context(|| format!("failed to open store at {store_path:?}"))?;

  // Build application state.
  let state = AppState {
    store:  Arc::new(store),
    auth:   Arc::new(AuthConfig {
      username:      server_cfg.auth_username.clone(),
      password_hash: server_cfg.auth_password_hash.clone(),
    }),
    config: Arc::new(server_cfg.clone()),
  };

  let app = kith_carddav::router(state);
  let address = format!("{}:{}", server_cfg.host, server_cfg.port);

  tracing::info!("Listening on http://{address}");
  let listener = TcpListener::bind(&address)
    .await
    .with_context(|| format!("failed to bind {address}"))?;

  axum::serve(listener, app).await.context("server error")?;

  Ok(())
}

/// Read a password from stdin (no echo).
fn rpassword_or_stdin() -> anyhow::Result<String> {
  use std::io::{self, BufRead, Write};
  let stdin = io::stdin();
  print!("Password: ");
  io::stdout().flush().ok();
  let mut line = String::new();
  stdin.lock().read_line(&mut line)?;
  Ok(
    line
      .trim_end_matches('\n')
      .trim_end_matches('\r')
      .to_string(),
  )
}

/// Expand a leading `~` to the user's home directory.
fn expand_tilde(path: &Path) -> PathBuf {
  let s = path.to_string_lossy();
  if let Some(rest) = s.strip_prefix("~/")
    && let Ok(home) = std::env::var("HOME")
  {
    return PathBuf::from(home).join(rest);
  }
  path.to_path_buf()
}
