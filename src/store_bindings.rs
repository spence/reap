//! Machine-local authority for named stores outside a project directory.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{config_path, Config};
use crate::manifest::Manifest;
use crate::stores::STORE_MARKER;
use crate::util::{lock_state, new_token, state_dir, write_json_atomic};

const MARKER_SIGNATURE: &str = "reap-external-store-v1";

/// Exact project, directory, and marker identity recorded on this machine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoreBinding {
  pub project_path: String,
  pub project_dev: u64,
  pub project_ino: u64,
  pub resource: String,
  pub path: String,
  pub dev: u64,
  pub ino: u64,
  pub token: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BindingFile {
  version: u32,
  bindings: Vec<StoreBinding>,
}

impl Default for BindingFile {
  fn default() -> Self {
    Self {
      version: 1,
      bindings: vec![],
    }
  }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExternalMarker {
  signature: String,
  binding: StoreBinding,
}

fn bindings_path(state: &Path) -> PathBuf {
  state.join("store-bindings.json")
}

fn load_bindings(state: &Path) -> Result<BindingFile, String> {
  let path = bindings_path(state);
  let text = match fs::read_to_string(&path) {
    Ok(text) => text,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BindingFile::default()),
    Err(e) => return Err(format!("{}: {e}", path.display())),
  };
  let file: BindingFile =
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
  if file.version != 1 {
    return Err(format!("{}: unsupported version", path.display()));
  }
  Ok(file)
}

fn project_identity(project: &Path) -> Result<(PathBuf, u64, u64), String> {
  let canon = fs::canonicalize(project).map_err(|e| format!("{}: {e}", project.display()))?;
  let meta = fs::symlink_metadata(&canon).map_err(|e| format!("{}: {e}", canon.display()))?;
  if !meta.is_dir() {
    return Err(format!("{} is not a project directory", canon.display()));
  }
  Ok((canon, meta.dev(), meta.ino()))
}

fn no_symlink_dir(path: &Path) -> Result<(PathBuf, u64, u64), String> {
  if !path.is_absolute() {
    return Err(format!("{} must be an absolute path", path.display()));
  }
  let mut walk = PathBuf::from("/");
  let mut previous_dev = Some(
    fs::symlink_metadata("/")
      .map_err(|e| format!("/: {e}"))?
      .dev(),
  );
  let mut transitions = 0;
  for component in path.components().skip(1) {
    let Component::Normal(name) = component else {
      return Err(format!("{} has a non-normal component", path.display()));
    };
    walk.push(name);
    let meta = fs::symlink_metadata(&walk).map_err(|e| format!("{}: {e}", walk.display()))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
      return Err(format!(
        "{} is a symlink or not a directory",
        walk.display()
      ));
    }
    if previous_dev.is_some_and(|dev| dev != meta.dev()) {
      transitions += 1;
    }
    previous_dev = Some(meta.dev());
  }
  if transitions > 1 {
    return Err(format!("{} crosses nested mounts", path.display()));
  }
  let canon = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
  if canon != path {
    return Err(format!("{} is not a canonical path", path.display()));
  }
  let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
  let parent = path
    .parent()
    .ok_or_else(|| format!("{} has no parent", path.display()))?;
  let parent_dev = fs::symlink_metadata(parent)
    .map_err(|e| format!("{}: {e}", parent.display()))?
    .dev();
  if meta.dev() != parent_dev {
    return Err(format!("{} is a mount root", path.display()));
  }
  Ok((canon, meta.dev(), meta.ino()))
}

fn overlaps(a: &Path, b: &Path) -> bool {
  a.starts_with(b) || b.starts_with(a)
}

fn strict_config() -> Result<Config, String> {
  let path = config_path();
  match fs::read_to_string(&path) {
    Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
    Err(e) => Err(format!("{}: {e}", path.display())),
  }
}

fn canonical_if_present(path: PathBuf) -> Result<PathBuf, String> {
  let mut current = path.as_path();
  let mut missing = Vec::new();
  loop {
    match fs::canonicalize(current) {
      Ok(mut canon) => {
        for name in missing.iter().rev() {
          canon.push(name);
        }
        return Ok(canon);
      }
      Err(e) if e.kind() == io::ErrorKind::NotFound => {
        let name = current
          .file_name()
          .ok_or_else(|| format!("{} cannot be resolved", path.display()))?;
        missing.push(name.to_os_string());
        current = current
          .parent()
          .ok_or_else(|| format!("{} has no parent", path.display()))?;
      }
      Err(e) => return Err(format!("{}: {e}", current.display())),
    }
  }
}

fn check_overlap(
  manifest: &Manifest,
  path: &Path,
  old: Option<&StoreBinding>,
  bindings: &[StoreBinding],
) -> Result<(), String> {
  let cfg = strict_config()?;
  let project = fs::canonicalize(&manifest.project_dir)
    .map_err(|e| format!("{}: {e}", manifest.project_dir.display()))?;
  let mut protected = vec![
    project.clone(),
    canonical_if_present(manifest.target_dir())?,
    canonical_if_present(state_dir())?,
    canonical_if_present(cfg.quarantine_dir())?,
  ];
  for root in cfg.expanded_roots() {
    protected.push(canonical_if_present(root)?);
  }
  for store in manifest
    .stores
    .iter()
    .filter(|store| store.resource.is_none())
  {
    protected.push(canonical_if_present(project.join(&store.path))?);
  }
  protected.extend(
    bindings
      .iter()
      .filter(|binding| old != Some(*binding))
      .map(|binding| PathBuf::from(&binding.path)),
  );
  if let Some(other) = protected.iter().find(|other| overlaps(path, other)) {
    return Err(format!(
      "{} overlaps protected root {}",
      path.display(),
      other.display()
    ));
  }
  let home = crate::config::home();
  if path == Path::new("/") || home.starts_with(path) {
    return Err(format!("{} is home or an ancestor", path.display()));
  }
  for name in [".git", ".reap.json", "Cargo.toml"] {
    let entry = path.join(name);
    match fs::symlink_metadata(&entry) {
      Ok(_) => return Err(format!("{} appears to be a project root", path.display())),
      Err(e) if e.kind() == io::ErrorKind::NotFound => {}
      Err(e) => return Err(format!("{}: {e}", entry.display())),
    }
  }
  Ok(())
}

fn marker_matches(binding: &StoreBinding) -> Result<(), String> {
  let path = Path::new(&binding.path).join(STORE_MARKER);
  let meta = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
  if !meta.is_file() {
    return Err(format!("{} is not a regular marker", path.display()));
  }
  let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
  let marker: ExternalMarker =
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
  if marker.signature != MARKER_SIGNATURE || marker.binding != *binding {
    return Err(format!(
      "{} does not match its local binding",
      path.display()
    ));
  }
  Ok(())
}

pub fn lookup(manifest: &Manifest, resource: &str) -> Result<Option<StoreBinding>, String> {
  let (project, dev, ino) = project_identity(&manifest.project_dir)?;
  let file = load_bindings(&state_dir())?;
  let mut matches = file.bindings.into_iter().filter(|binding| {
    binding.project_path == project.to_string_lossy()
      && binding.project_dev == dev
      && binding.project_ino == ino
      && binding.resource == resource
  });
  let binding = matches.next();
  if matches.next().is_some() {
    return Err(format!(
      "duplicate local bindings for resource {resource:?}"
    ));
  }
  Ok(binding)
}

pub fn verify(binding: &StoreBinding) -> Result<PathBuf, String> {
  let path = Path::new(&binding.path);
  let (canon, dev, ino) = no_symlink_dir(path)?;
  if dev != binding.dev || ino != binding.ino || canon.to_string_lossy() != binding.path {
    return Err(format!("{} changed identity", path.display()));
  }
  let project = Path::new(&binding.project_path);
  let (canon_project, project_dev, project_ino) = project_identity(project)?;
  if canon_project.to_string_lossy() != binding.project_path
    || project_dev != binding.project_dev
    || project_ino != binding.project_ino
  {
    return Err(format!("{} changed project identity", project.display()));
  }
  marker_matches(binding)?;
  Ok(canon)
}

pub fn verify_for_manifest(manifest: &Manifest, binding: &StoreBinding) -> Result<PathBuf, String> {
  let path = verify(binding)?;
  let file = load_bindings(&state_dir())?;
  if !file.bindings.contains(binding) {
    return Err(format!(
      "resource {:?} no longer has this local binding",
      binding.resource
    ));
  }
  check_overlap(manifest, &path, Some(binding), &file.bindings)?;
  Ok(path)
}

fn write_marker(binding: &StoreBinding) -> Result<(), String> {
  let path = Path::new(&binding.path).join(STORE_MARKER);
  match fs::symlink_metadata(&path) {
    Ok(_) => return Err(format!("{} already has a store marker", path.display())),
    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
    Err(e) => return Err(format!("{}: {e}", path.display())),
  }
  let temp = Path::new(&binding.path).join(format!(
    ".{STORE_MARKER}.{}-{}.tmp",
    std::process::id(),
    &binding.token[..8]
  ));
  let marker = ExternalMarker {
    signature: MARKER_SIGNATURE.to_string(),
    binding: binding.clone(),
  };
  let mut text = serde_json::to_vec_pretty(&marker).map_err(|e| e.to_string())?;
  text.push(b'\n');
  let mut file = OpenOptions::new()
    .write(true)
    .create_new(true)
    .open(&temp)
    .map_err(|e| format!("{}: {e}", temp.display()))?;
  let result = file
    .write_all(&text)
    .and_then(|_| file.sync_all())
    .and_then(|_| fs::rename(&temp, &path));
  if result.is_err() {
    let _ = fs::remove_file(&temp);
  }
  result.map_err(|e| format!("{}: {e}", path.display()))
}

pub fn bind(manifest: &Manifest, resource: &str, path: &Path) -> Result<PathBuf, String> {
  if !manifest
    .stores
    .iter()
    .any(|store| store.resource.as_deref() == Some(resource))
  {
    return Err(format!(
      "resource {resource:?} is not declared in .reap.json"
    ));
  }
  let state = state_dir();
  let _lock = lock_state(&state).map_err(|e| format!("{}: {e}", state.display()))?;
  let mut file = load_bindings(&state)?;
  let (project, project_dev, project_ino) = project_identity(&manifest.project_dir)?;
  let (canon, dev, ino) = no_symlink_dir(path)?;
  let old = file.bindings.iter().find(|binding| {
    binding.project_path == project.to_string_lossy() && binding.resource == resource
  });
  check_overlap(manifest, &canon, old, &file.bindings)?;
  if let Some(old) = old {
    if old.path == canon.to_string_lossy()
      && old.dev == dev
      && old.ino == ino
      && old.project_dev == project_dev
      && old.project_ino == project_ino
      && verify_for_manifest(manifest, old).is_ok()
    {
      return Ok(canon);
    }
  }
  let binding = StoreBinding {
    project_path: project.to_string_lossy().into_owned(),
    project_dev,
    project_ino,
    resource: resource.to_string(),
    path: canon.to_string_lossy().into_owned(),
    dev,
    ino,
    token: new_token(&format!("{}:{resource}", canon.display())),
  };
  write_marker(&binding)?;
  file.bindings.retain(|current| {
    !(current.project_path == binding.project_path && current.resource == resource)
  });
  file.bindings.push(binding);
  write_json_atomic(&bindings_path(&state), &file)
    .map_err(|e| format!("{}: {e}", bindings_path(&state).display()))?;
  Ok(canon)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn mount_root_cannot_be_bound_as_a_store() {
    let root_dev = fs::symlink_metadata("/").unwrap().dev();
    for path in ["/dev", "/Volumes/kytos"] {
      if fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir() && meta.dev() != root_dev) {
        assert!(no_symlink_dir(Path::new(path)).is_err());
        return;
      }
    }
  }
}
