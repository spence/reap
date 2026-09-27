//! reap's global config: where to look for cargo target dirs, and what to skip.
//!
//! No config file -> built-in defaults (`roots = ["~/src"]`). This replaces the
//! old per-project registry: discovery under these roots finds every target dir,
//! so nothing needs to register itself.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
  pub roots: Vec<String>,
  /// Shallow, read-only audit roots; absent means the discovery roots.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub coverage_roots: Option<Vec<String>>,
  pub exclude: Vec<String>,
  pub quarantine: QuarantineConfig,
}

impl Default for Config {
  fn default() -> Self {
    Config {
      roots: vec!["~/src".to_string()],
      coverage_roots: None,
      exclude: vec![],
      quarantine: QuarantineConfig::default(),
    }
  }
}

impl Config {
  pub fn expanded_roots(&self) -> Vec<PathBuf> {
    self.roots.iter().map(|r| expand(r)).collect()
  }

  pub fn expanded_coverage_roots(&self) -> Vec<PathBuf> {
    self
      .coverage_roots
      .as_ref()
      .unwrap_or(&self.roots)
      .iter()
      .map(|root| expand(root))
      .collect()
  }

  /// Resolved quarantine location: configured `quarantine.dir` (e.g. an
  /// external drive) or the machine-local default under the state dir.
  pub fn quarantine_dir(&self) -> PathBuf {
    match &self.quarantine.dir {
      Some(s) if !s.trim().is_empty() => expand(s),
      _ => crate::util::state_dir().join("quarantine"),
    }
  }
}

/// Per-machine quarantine policy. `dir: null` -> `<state dir>/quarantine`.
/// With `auto_purge: false`, a bare `reap purge` deletes nothing on this
/// machine -- only explicit selectors (`--id`/`--owner`/`--all`) purge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QuarantineConfig {
  pub dir: Option<String>,
  pub auto_purge: bool,
  pub purge_after_days: u32,
}

impl Default for QuarantineConfig {
  fn default() -> Self {
    QuarantineConfig {
      dir: None,
      auto_purge: true,
      purge_after_days: 30,
    }
  }
}

pub fn config_path() -> PathBuf {
  home().join(".config/reap/config.json")
}

/// Returns the config and whether it came from a file (vs built-in defaults).
pub fn load_config() -> (Config, bool) {
  if let Ok(snapshot) = std::env::var("REAP_INTERNAL_CONFIG_SNAPSHOT") {
    return match serde_json::from_str::<Config>(&snapshot) {
      Ok(cfg) => (cfg, true),
      Err(e) => {
        eprintln!("error: invalid maintenance config snapshot: {e}");
        std::process::exit(1);
      }
    };
  }
  if let Ok(text) = fs::read_to_string(config_path()) {
    if let Ok(cfg) = serde_json::from_str::<Config>(&text) {
      return (cfg, true);
    }
  }
  (Config::default(), false)
}

/// Write the default config file. Returns `Ok(false)` if it already exists.
pub fn write_default_config() -> std::io::Result<bool> {
  let path = config_path();
  if path.exists() {
    return Ok(false);
  }
  if let Some(dir) = path.parent() {
    fs::create_dir_all(dir)?;
  }
  let mut text = serde_json::to_string_pretty(&Config::default()).unwrap_or_default();
  text.push('\n');
  let tmp = path.with_extension("json.tmp");
  fs::write(&tmp, text)?;
  fs::rename(&tmp, &path)?;
  Ok(true)
}

pub(crate) fn home() -> PathBuf {
  PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

/// Expand a leading `~`/`~/` and `$HOME`/`${HOME}` in a config path string.
pub fn expand(s: &str) -> PathBuf {
  let home = home().to_string_lossy().into_owned();
  let mut t = if s == "~" {
    home.clone()
  } else if let Some(rest) = s.strip_prefix("~/") {
    format!("{}/{}", home, rest)
  } else {
    s.to_string()
  };
  if t.contains("$HOME") {
    t = t.replace("${HOME}", &home).replace("$HOME", &home);
  }
  PathBuf::from(t)
}
