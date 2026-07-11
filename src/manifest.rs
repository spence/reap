//! Per-project manifest (`.reap.json`): the exception-declaring config a project
//! carries ONLY when it has non-regenerable artifacts inside the prunable subdirs,
//! or wants a non-default policy.
//!
//! Every field is optional and falls back to a safe built-in default, so most
//! projects need no manifest at all -- structural containment + the min-age guard
//! do the protecting either way. reap never scaffolds one; its presence is signal.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const MANIFEST_NAME: &str = ".reap.json";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Keep {
  pub profiles: Vec<String>,
  pub paths: Vec<String>,
  pub names: Vec<String>,
  pub binaries: Vec<String>,
}

impl Default for Keep {
  fn default() -> Self {
    Keep {
      profiles: vec!["release".to_string()],
      paths: vec![],
      names: vec![],
      binaries: vec![],
    }
  }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Policy {
  pub keep_recent: usize,
  pub prune_incremental: bool,
  pub prune_build_scripts: bool,
  pub min_age_minutes: f64,
  pub stale_profile_days: BTreeMap<String, i64>,
}

impl Default for Policy {
  fn default() -> Self {
    Policy {
      keep_recent: 1,
      prune_incremental: true,
      prune_build_scripts: true,
      min_age_minutes: 10.0,
      stale_profile_days: BTreeMap::new(),
    }
  }
}

/// The on-disk shape of `.reap.json`. Unknown fields are ignored (tolerant
/// reader); missing fields fall back to the defaults above.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct ManifestFile {
  version: u32,
  target: String,
  keep: Keep,
  policy: Policy,
}

impl Default for ManifestFile {
  fn default() -> Self {
    ManifestFile {
      version: 1,
      target: "target".to_string(),
      keep: Keep::default(),
      policy: Policy::default(),
    }
  }
}

/// A fully-resolved manifest bound to a project directory.
#[derive(Debug, Clone)]
pub struct Manifest {
  pub project_dir: PathBuf,
  pub target: String,
  pub keep: Keep,
  pub policy: Policy,
  pub has_file: bool,
}

#[derive(Debug)]
pub struct ManifestError(pub String);

impl fmt::Display for ManifestError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}", self.0)
  }
}
impl std::error::Error for ManifestError {}

impl Manifest {
  pub fn target_dir(&self) -> PathBuf {
    let t = Path::new(&self.target);
    if t.is_absolute() {
      t.to_path_buf()
    } else {
      self.project_dir.join(t)
    }
  }
}

pub fn load_manifest(project_dir: &Path) -> Result<Manifest, ManifestError> {
  let path = project_dir.join(MANIFEST_NAME);
  let has_file = path.is_file();
  let file: ManifestFile = if has_file {
    let text =
      fs::read_to_string(&path).map_err(|e| ManifestError(format!("{}: {}", path.display(), e)))?;
    serde_json::from_str(&text).map_err(|e| ManifestError(format!("{}: {}", path.display(), e)))?
  } else {
    ManifestFile::default()
  };
  let mut policy = file.policy;
  policy.keep_recent = policy.keep_recent.max(1);
  if policy.min_age_minutes.is_nan() || policy.min_age_minutes < 0.0 {
    policy.min_age_minutes = 0.0;
  }
  Ok(Manifest {
    project_dir: project_dir.to_path_buf(),
    target: file.target,
    keep: file.keep,
    policy,
    has_file,
  })
}

/// Walk up from `start` to the nearest dir with a `.reap.json` or `Cargo.toml`.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
  let mut cur = start.to_path_buf();
  loop {
    if cur.join(MANIFEST_NAME).is_file() || cur.join("Cargo.toml").is_file() {
      return Some(cur);
    }
    cur = cur.parent()?.to_path_buf();
  }
}
