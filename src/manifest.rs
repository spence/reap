//! Per-project manifest (`.reap.json`): the exception-declaring config a project
//! carries ONLY when it has non-regenerable artifacts inside the prunable subdirs,
//! or wants a non-default policy.
//!
//! Every field is optional and falls back to a safe built-in default, so most
//! projects need no manifest at all -- structural containment + the min-age guard
//! do the protecting either way. reap never scaffolds one; its presence is signal.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Deserializer};

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

/// A declared artifact store (`"version": 2`). The direct children of a
/// project-relative directory or locally bound named external resource
/// (benchmark runs, log batches) may be deleted under
/// `retention`. Parsed strictly -- an unknown field here is an error, because
/// a typo'd protection must not silently vanish from destructive policy.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Store {
  #[serde(default)]
  pub path: String,
  pub resource: Option<String>,
  #[serde(default = "default_unit")]
  pub unit: String,
  pub retention: Retention,
  #[serde(default)]
  pub disposition: StoreDisposition,
  #[serde(default, deserialize_with = "declared_series")]
  pub series: Option<Vec<StoreSeries>>,
}

/// Whether eligible store units are deleted now or moved into quarantine.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoreDisposition {
  #[default]
  Delete,
  Quarantine,
}

/// A named sequence of direct store children selected by one `*` in a basename.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreSeries {
  pub name: String,
  pub pattern: String,
}

/// Store retention. `keep_last` and `min_age_hours` are unconditional
/// protections; `max_age_days` / `max_bytes` are the only deletion triggers,
/// and at least one is required.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Retention {
  pub keep_last: usize,
  pub min_age_hours: f64,
  pub max_age_days: Option<f64>,
  pub max_bytes: Option<u64>,
}

impl Default for Retention {
  fn default() -> Self {
    Retention {
      keep_last: 1,
      min_age_hours: 24.0,
      max_age_days: None,
      max_bytes: None,
    }
  }
}

/// The on-disk shape of `.reap.json`. Unknown top-level fields are ignored
/// (tolerant reader, v1 compat); the destructive `stores` subtree is strict.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct ManifestFile {
  version: u32,
  target: String,
  keep: Keep,
  policy: Policy,
  stores: Vec<Store>,
}

impl Default for ManifestFile {
  fn default() -> Self {
    ManifestFile {
      version: 1,
      target: "target".to_string(),
      keep: Keep::default(),
      policy: Policy::default(),
      stores: vec![],
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
  pub stores: Vec<Store>,
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
  if file.version > 2 {
    return Err(ManifestError(format!(
      "{}: unsupported version {} (this reap understands 1 and 2)",
      path.display(),
      file.version
    )));
  }
  if !file.stores.is_empty() {
    if file.version < 2 {
      return Err(ManifestError(format!(
        "{}: \"stores\" requires \"version\": 2",
        path.display()
      )));
    }
    validate_stores(&file.stores, &file.target)
      .map_err(|e| ManifestError(format!("{}: {}", path.display(), e)))?;
  }
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
    stores: file.stores,
    has_file,
  })
}

/// Static validation of store declarations (destructive policy fails closed):
/// exact project-relative paths, no globs or `..`, no overlap with each other
/// or with the target dir, and at least one retention trigger.
fn validate_stores(stores: &[Store], target: &str) -> Result<(), String> {
  let mut seen: Vec<PathBuf> = Vec::new();
  let mut resources = HashSet::new();
  for s in stores {
    let p = if let Some(resource) = &s.resource {
      if !s.path.is_empty() {
        return Err(format!(
          "store {resource:?} cannot have both path and resource"
        ));
      }
      if resource.is_empty()
        || !resource
          .bytes()
          .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        || !resources.insert(resource)
      {
        return Err(format!("bad or duplicate store resource name {resource:?}"));
      }
      None
    } else {
      let raw = s.path.trim_matches('/');
      if raw.is_empty() {
        return Err("store path is empty".to_string());
      }
      if s.path.starts_with('/') {
        return Err(format!("store path {:?} must be project-relative", s.path));
      }
      if s.path.contains('*') || s.path.contains('?') || s.path.contains('[') {
        return Err(format!("store path {:?} must be exact (no globs)", s.path));
      }
      let p = PathBuf::from(raw);
      if p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(format!(
          "store path {:?} must not contain '.' or '..'",
          s.path
        ));
      }
      Some(p)
    };
    if s.unit != "children" {
      return Err(format!(
        "store {:?}: unsupported unit {:?} (only \"children\")",
        s.path, s.unit
      ));
    }
    let r = &s.retention;
    if r.max_age_days.is_none() && r.max_bytes.is_none() {
      return Err(format!(
        "store {:?}: retention needs max_age_days and/or max_bytes",
        s.path
      ));
    }
    if !r.min_age_hours.is_finite() || r.min_age_hours < 0.0 {
      return Err(format!("store {:?}: bad min_age_hours", s.path));
    }
    if matches!(r.max_age_days, Some(d) if !d.is_finite() || d < 0.0) {
      return Err(format!("store {:?}: bad max_age_days", s.path));
    }
    let mut series_names = HashSet::new();
    let mut series_patterns = HashSet::new();
    if matches!(&s.series, Some(series) if series.is_empty()) {
      return Err(format!(
        "store {:?}: declared series cannot be empty",
        s.path
      ));
    }
    for series in s.series.iter().flatten() {
      if series.name.is_empty()
        || !series
          .name
          .bytes()
          .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        || !series_names.insert(&series.name)
      {
        return Err(format!("store {:?}: bad or duplicate series name", s.path));
      }
      let parts: Vec<&str> = series.pattern.split('*').collect();
      if parts.len() != 2
        || (parts[0].is_empty() && parts[1].is_empty())
        || series.pattern.contains(['/', '\\', '?', '[', ']'])
        || !series_patterns.insert(&series.pattern)
      {
        return Err(format!(
          "store {:?}: bad or duplicate series pattern {:?} (use one '*' in a basename)",
          s.path, series.pattern
        ));
      }
    }
    if let Some(p) = p {
      let t = Path::new(target);
      if !t.is_absolute() && (p.starts_with(t) || t.starts_with(&p)) {
        return Err(format!("store path {:?} overlaps the target dir", s.path));
      }
      for prev in &seen {
        if p.starts_with(prev) || prev.starts_with(&p) {
          return Err(format!(
            "store paths {:?} and {:?} overlap",
            prev.display(),
            p.display()
          ));
        }
      }
      seen.push(p);
    }
  }
  Ok(())
}

fn default_unit() -> String {
  "children".to_string()
}

fn declared_series<'de, D>(deserializer: D) -> Result<Option<Vec<StoreSeries>>, D::Error>
where
  D: Deserializer<'de>,
{
  Vec::<StoreSeries>::deserialize(deserializer).map(Some)
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

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::{AtomicUsize, Ordering};

  static N: AtomicUsize = AtomicUsize::new(0);

  fn write_manifest(json: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("reap-man-{}-{}", std::process::id(), n));
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join(MANIFEST_NAME), json).unwrap();
    d
  }

  #[test]
  fn stores_parse_and_version_gate() {
    let ok = write_manifest(
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"keep_last":3,"max_age_days":30}}]}"#,
    );
    let m = load_manifest(&ok).unwrap();
    assert_eq!(m.stores.len(), 1);
    assert_eq!(m.stores[0].retention.keep_last, 3);
    assert_eq!(m.stores[0].unit, "children");
    assert!(m.stores[0].series.is_none(), "v2 default remains global");
    assert_eq!(m.stores[0].disposition, StoreDisposition::Delete);
    let _ = fs::remove_dir_all(&ok);

    let named = write_manifest(
      r#"{"version":2,"stores":[{"path":"bench/results","retention":{"max_age_days":30},"series":[{"name":"run","pattern":"run.*"}]}]}"#,
    );
    let m = load_manifest(&named).unwrap();
    assert_eq!(m.stores[0].series.as_ref().unwrap()[0].name, "run");
    assert_eq!(m.stores[0].series.as_ref().unwrap()[0].pattern, "run.*");
    let _ = fs::remove_dir_all(named);

    let external = write_manifest(
      r#"{"version":2,"stores":[{"resource":"logs","retention":{"max_age_days":1}}]}"#,
    );
    let m = load_manifest(&external).unwrap();
    assert_eq!(m.stores[0].resource.as_deref(), Some("logs"));
    assert!(m.stores[0].path.is_empty());
    let _ = fs::remove_dir_all(external);

    let quarantined = write_manifest(
      r#"{"version":2,"stores":[{"path":"logs","disposition":"quarantine","retention":{"max_age_days":1}}]}"#,
    );
    let m = load_manifest(&quarantined).unwrap();
    assert_eq!(m.stores[0].disposition, StoreDisposition::Quarantine);
    let _ = fs::remove_dir_all(quarantined);

    for (bad, why) in [
      (
        r#"{"stores":[{"path":"x","retention":{"max_age_days":1}}]}"#,
        "stores without version 2",
      ),
      (r#"{"version":3}"#, "unknown future version"),
      (
        r#"{"version":2,"stores":[{"path":"x","retention":{"keep_lastt":3,"max_age_days":1}}]}"#,
        "unknown retention field",
      ),
      (
        r#"{"version":2,"stores":[{"path":"x","retention":{"keep_last":3}}]}"#,
        "retention without a trigger",
      ),
      (
        r#"{"version":2,"stores":[{"path":"x","retention":{"max_age_days":1},"series":[{"name":"run","pattern":"run.?"}]}]}"#,
        "unknown pattern syntax",
      ),
      (
        r#"{"version":2,"stores":[{"path":"x","retention":{"max_age_days":1},"series":[{"name":"run","pattern":"run.**"}]}]}"#,
        "multiple wildcard tokens",
      ),
      (
        r#"{"version":2,"stores":[{"path":"x","retention":{"max_age_days":1},"series":[{"name":"run","pattern":"*"}]}]}"#,
        "unbounded wildcard",
      ),
      (
        r#"{"version":2,"stores":[{"path":"x","retention":{"max_age_days":1},"series":[]}]}"#,
        "explicit empty series",
      ),
      (
        r#"{"version":2,"stores":[{"path":"x","retention":{"max_age_days":1},"series":null}]}"#,
        "explicit null series",
      ),
      (
        r#"{"version":2,"stores":[{"path":"x","retention":{"max_age_days":1},"series":[{"name":"run","pattern":"run.*"},{"name":"run","pattern":"other.*"}]}]}"#,
        "duplicate series name",
      ),
      (
        r#"{"version":2,"stores":[{"path":"x","retention":{"max_age_days":1},"series":[{"name":"run","pattern":"run.*","keep_last":3}]}]}"#,
        "unknown series field",
      ),
      (
        r#"{"version":2,"stores":[{"resource":"logs","path":"logs","retention":{"max_age_days":1}}]}"#,
        "external resource with project-relative path",
      ),
      (
        r#"{"version":2,"stores":[{"resource":"bad/name","retention":{"max_age_days":1}}]}"#,
        "bad resource name",
      ),
      (
        r#"{"version":2,"stores":[{"resource":"logs","retention":{"max_age_days":1}},{"resource":"logs","retention":{"max_age_days":1}}]}"#,
        "duplicate resource name",
      ),
      (
        r#"{"version":2,"stores":[{"path":"logs","disposition":"archive","retention":{"max_age_days":1}}]}"#,
        "unknown store disposition",
      ),
    ] {
      let d = write_manifest(bad);
      assert!(load_manifest(&d).is_err(), "must reject: {}", why);
      let _ = fs::remove_dir_all(&d);
    }
  }

  #[test]
  fn store_path_safety() {
    for bad in [
      r#"{"version":2,"stores":[{"path":"/abs","retention":{"max_age_days":1}}]}"#,
      r#"{"version":2,"stores":[{"path":"../up","retention":{"max_age_days":1}}]}"#,
      r#"{"version":2,"stores":[{"path":"a/*","retention":{"max_age_days":1}}]}"#,
      r#"{"version":2,"stores":[{"path":"target/logs","retention":{"max_age_days":1}}]}"#,
      r#"{"version":2,"stores":[{"path":"a","retention":{"max_age_days":1}},{"path":"a/b","retention":{"max_age_days":1}}]}"#,
      r#"{"version":2,"stores":[{"path":"x","unit":"tree","retention":{"max_age_days":1}}]}"#,
    ] {
      let d = write_manifest(bad);
      assert!(load_manifest(&d).is_err(), "must reject: {}", bad);
      let _ = fs::remove_dir_all(&d);
    }

    // v1 manifests keep working and stay tolerant of unknown top-level fields.
    let v1 = write_manifest(r#"{"keep":{"paths":["prebuilt/"]},"future_field":1}"#);
    let m = load_manifest(&v1).unwrap();
    assert!(m.stores.is_empty());
    assert_eq!(m.keep.paths, vec!["prebuilt/"]);
    let _ = fs::remove_dir_all(&v1);
  }
}
