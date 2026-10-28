//! Complete cleanup planning evidence and exact retirement of known owned removals.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::files::{descriptor_stat, for_each_preview_name, open_physical, open_relative, stat_at};

const MAX_ENTRIES: usize = 131072;

/// Metadata that can affect selection, namespace bindings, or protected source bytes.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
struct Stamp {
  device: u64,
  inode: u64,
  mode: u32,
  uid: u32,
  gid: u32,
  links: u64,
  size: i64,
  modified: (i64, i64),
  changed: (i64, i64),
}

impl Stamp {
  fn read(value: &libc::stat) -> Self {
    Self {
      device: value.st_dev as u64,
      inode: value.st_ino as u64,
      mode: value.st_mode as u32,
      uid: value.st_uid,
      gid: value.st_gid,
      links: value.st_nlink as u64,
      size: value.st_size as i64,
      modified: (value.st_mtime as i64, value.st_mtime_nsec as i64),
      changed: (value.st_ctime as i64, value.st_ctime_nsec as i64),
    }
  }

  fn directory(&self) -> bool {
    self.mode & libc::S_IFMT as u32 == libc::S_IFDIR as u32
  }

  fn same_object(&self, other: &Self) -> bool {
    self.device == other.device
      && self.inode == other.inode
      && self.mode == other.mode
      && self.uid == other.uid
      && self.gid == other.gid
  }
}

/// A unit snapshot excludes other store units; retention evaluates them before that unit starts.
#[derive(Clone, serde::Serialize)]
enum Coverage {
  Cargo,
  Unit { name: PathBuf },
}

impl Coverage {
  fn includes(&self, root: &Path, path: &Path) -> bool {
    let Self::Unit { name } = self else {
      return true;
    };
    path == root
      || path
        .strip_prefix(root)
        .ok()
        .and_then(|path| path.components().next())
        .is_some_and(|part| part.as_os_str() == name.as_os_str())
  }

  fn same(&self, root: &Path, path: &Path, before: &Stamp, after: &Stamp) -> bool {
    if matches!(self, Self::Unit { .. }) && path == root {
      before.same_object(after)
    } else {
      before == after
    }
  }
}

/// Complete finite selection inputs, not permission to remove them. Child links remain leaves.
pub(super) struct Inventory {
  root: PathBuf,
  nodes: BTreeMap<PathBuf, Stamp>,
  /// Bind the initial inventory and its exact owned-removal prefix, not cached freshness.
  evidence: Sha256,
  coverage: Coverage,
}

impl Inventory {
  /// Bind the complete selection and owned-removal history without another inventory scan.
  pub(super) fn bind(&self, digest: &mut Sha256) {
    digest.update(self.evidence.clone().finalize());
  }

  pub(super) fn capture(root: &Path) -> io::Result<Self> {
    Self::capture_with(root, Coverage::Cargo)
  }

  /// Capture one direct child and its descendants. Parent identity stays fixed; other units
  /// remain outside this snapshot and must be evaluated through current retention policy.
  pub(super) fn capture_unit(parent: &Path, source: &Path) -> io::Result<Self> {
    if source.parent() != Some(parent) {
      return Err(io::Error::other("cleanup unit is not a direct child"));
    }
    let name = source
      .file_name()
      .ok_or_else(|| io::Error::other("cleanup unit has no name"))?;
    let inventory = Self::capture_with(
      parent,
      Coverage::Unit {
        name: PathBuf::from(name),
      },
    )?;
    if !inventory.nodes.contains_key(source) {
      return Err(io::Error::other("cleanup unit is absent"));
    }
    Ok(inventory)
  }

  fn capture_with(root: &Path, coverage: Coverage) -> io::Result<Self> {
    let nodes = Self::observe(root, &coverage)?;
    let mut evidence = Sha256::new();
    evidence.update(b"reap.cargo.inventory.v1");
    if matches!(coverage, Coverage::Unit { .. }) {
      evidence.update(serde_json::to_vec(&coverage)?);
    }
    for (path, stamp) in &nodes {
      bind_entry(&mut evidence, path, stamp)?;
    }
    Ok(Self {
      root: root.into(),
      nodes,
      evidence,
      coverage,
    })
  }

  fn observe(root: &Path, coverage: &Coverage) -> io::Result<BTreeMap<PathBuf, Stamp>> {
    let directory = open_physical(root)?;
    let first = Stamp::read(&descriptor_stat(&directory)?);
    let mut nodes = BTreeMap::from([(root.to_path_buf(), first.clone())]);
    let mut pending = vec![root.to_path_buf()];
    let mut observed_names = 0usize;
    while let Some(path) = pending.pop() {
      let held = open_relative(&directory, root, &path, first.device)?;
      let before = Stamp::read(&descriptor_stat(&held)?);
      if !nodes
        .get(&path)
        .is_some_and(|prior| coverage.same(root, &path, prior, &before))
        || !before.directory()
      {
        return Err(io::Error::other(
          "cleanup directory changed during inventory",
        ));
      }
      for_each_preview_name(&held, |name| {
        observed_names += 1;
        if observed_names > MAX_ENTRIES {
          return Err(io::Error::other("cleanup inventory exceeds its name bound"));
        }
        let name = Path::new(std::ffi::OsStr::from_bytes(name));
        let child = path.join(name);
        if !coverage.includes(root, &child) {
          return Ok(());
        }
        if nodes.len() >= MAX_ENTRIES {
          return Err(io::Error::other(
            "cleanup inventory exceeds its entry bound",
          ));
        }
        let observed = stat_at(&held, name)?;
        let kind = observed.st_mode & libc::S_IFMT;
        if observed.st_dev as u64 != first.device
          || (matches!(coverage, Coverage::Cargo)
            && !matches!(kind, libc::S_IFDIR | libc::S_IFREG | libc::S_IFLNK))
        {
          return Err(io::Error::other(
            "cleanup inventory crosses a mount or has a special entry",
          ));
        }
        if nodes
          .insert(child.clone(), Stamp::read(&observed))
          .is_some()
        {
          return Err(io::Error::other("cleanup inventory repeats an entry"));
        }
        if kind == libc::S_IFDIR {
          pending.push(child);
        }
        Ok(())
      })?;
      if !coverage.same(root, &path, &before, &Stamp::read(&descriptor_stat(&held)?)) {
        return Err(io::Error::other(
          "cleanup directory changed during inventory",
        ));
      }
    }
    Ok(nodes)
  }

  /// Refuse additions, replacements, content changes and incomplete namespace observations.
  /// Compare every entry without constructing another complete inventory.
  pub(super) fn fresh(&self) -> io::Result<()> {
    let directory = open_physical(&self.root)?;
    let device = self.nodes[&self.root].device;
    let mut count = 1;
    let mut observed_names = 0usize;
    for (path, before) in self.nodes.iter().filter(|(_, stamp)| stamp.directory()) {
      let held = open_relative(&directory, &self.root, path, device)?;
      if !self.coverage.same(
        &self.root,
        path,
        before,
        &Stamp::read(&descriptor_stat(&held)?),
      ) {
        return Err(io::Error::other("cleanup planning input changed"));
      }
      let mut names = BTreeSet::new();
      for_each_preview_name(&held, |name| {
        observed_names += 1;
        if observed_names > MAX_ENTRIES {
          return Err(io::Error::other("cleanup inventory exceeds its name bound"));
        }
        let name = Path::new(std::ffi::OsStr::from_bytes(name));
        let child = path.join(name);
        if !self.coverage.includes(&self.root, &child) {
          return Ok(());
        }
        if count >= self.nodes.len() || !names.insert(name.as_os_str().as_bytes().to_vec()) {
          return Err(io::Error::other("cleanup planning namespace changed"));
        }
        let observed = Stamp::read(&stat_at(&held, name)?);
        if self.nodes.get(&child) != Some(&observed) {
          return Err(io::Error::other("cleanup planning input changed"));
        }
        count += 1;
        Ok(())
      })?;
      if !self.coverage.same(
        &self.root,
        path,
        before,
        &Stamp::read(&descriptor_stat(&held)?),
      ) {
        return Err(io::Error::other(
          "cleanup directory changed during inventory",
        ));
      }
    }
    if count != self.nodes.len() {
      return Err(io::Error::other("cleanup planning input changed"));
    }
    Ok(())
  }

  /// Prepare a fixed postorder list. Policy entries follow all other children at their depth.
  pub(super) fn entries(&self, root: &Path) -> io::Result<Vec<PathBuf>> {
    if !self.nodes.contains_key(root) {
      return Err(io::Error::other(
        "cleanup selection is outside its captured inventory",
      ));
    }
    let mut paths: Vec<_> = self
      .nodes
      .range::<Path, _>((std::ops::Bound::Included(root), std::ops::Bound::Unbounded))
      .take_while(|(path, _)| path.starts_with(root))
      .map(|(path, _)| path.clone())
      .collect();
    if paths.iter().any(|path|!matches!(self.nodes[path].mode & libc::S_IFMT as u32,kind if kind==libc::S_IFDIR as u32 || kind==libc::S_IFREG as u32 || kind==libc::S_IFLNK as u32)) {
      return Err(io::Error::other("selected cleanup unit has an opaque entry"));
    }
    paths.sort_by(|left, right| {
      right
        .components()
        .count()
        .cmp(&left.components().count())
        .then_with(|| {
          (left.file_name() == Some(std::ffi::OsStr::new(crate::lifetime::FILE_NAME)))
            .cmp(&(right.file_name() == Some(std::ffi::OsStr::new(crate::lifetime::FILE_NAME))))
        })
        .then_with(|| left.cmp(right))
    });
    Ok(paths)
  }

  /// Compare the held source with its original observation before any intent is requested.
  pub(super) fn source(&self, path: &Path, held: &File) -> io::Result<()> {
    if self.nodes.get(path) != Some(&Stamp::read(&descriptor_stat(held)?)) {
      return Err(io::Error::other(
        "cleanup source differs from its selection",
      ));
    }
    Ok(())
  }

  /// Retire exactly one completed unlink. Only its parent and matching hard-link metadata can
  /// change. A later mismatch stops work; it does not erase the already recorded unit effect.
  pub(super) fn removed(&mut self, path: &Path, original: &File) -> io::Result<()> {
    self.retired(path, original, false)
  }

  /// Retire one atomically renamed subtree. Other entries must match the original inventory.
  pub(super) fn renamed(&mut self, path: &Path, original: &File) -> io::Result<()> {
    self.retired(path, original, true)
  }

  fn retired(&mut self, path: &Path, original: &File, renamed: bool) -> io::Result<()> {
    let before = self
      .nodes
      .get(path)
      .ok_or_else(|| io::Error::other("removed cleanup source was not inventoried"))?;
    let after = Stamp::read(&descriptor_stat(original)?);
    let mut expected_source = before.clone();
    expected_source.changed = after.changed;
    expected_source.links = if renamed {
      before.links
    } else if before.directory() {
      after.links
    } else {
      before
        .links
        .checked_sub(1)
        .ok_or_else(|| io::Error::other("cleanup source had no links"))?
    };
    if before.directory() && !renamed {
      expected_source.size = after.size;
    }
    if after != expected_source {
      return Err(io::Error::other("cleanup source changed during its unlink"));
    }
    let current = Self::observe(&self.root, &self.coverage)?;
    let parent = path
      .parent()
      .ok_or_else(|| io::Error::other("cleanup source has no parent"))?;
    let prior_parent = self
      .nodes
      .get(parent)
      .ok_or_else(|| io::Error::other("cleanup source parent was not inventoried"))?;
    let next_parent = current
      .get(parent)
      .filter(|next| prior_parent.same_object(next) && next.directory())
      .ok_or_else(|| io::Error::other("cleanup source parent changed identity"))?;
    let retired = self
      .nodes
      .keys()
      .filter(|name| **name == path || (renamed && name.starts_with(path)))
      .count();
    if current.len() + retired != self.nodes.len()
      || current.contains_key(path)
      || self.nodes.iter().any(|(name, prior)| {
        if name == path || (renamed && name.starts_with(path)) {
          return false;
        }
        let mut expected = prior.clone();
        if name == parent {
          expected = next_parent.clone();
        }
        if expected.device == before.device && expected.inode == before.inode {
          expected.changed = after.changed;
          expected.links = after.links;
        }
        !current
          .get(name)
          .is_some_and(|actual| self.coverage.same(&self.root, name, &expected, actual))
      })
    {
      return Err(io::Error::other(
        "cleanup input differs from its exact owned removal prefix",
      ));
    }

    // the prior commitment identifies every alias; this delta binds its exact permitted changes
    let mut evidence = self.evidence.clone();
    evidence.update(if renamed {
      b"reap.cleanup.inventory.renamed.v1".as_slice()
    } else {
      b"reap.cargo.inventory.removed.v1".as_slice()
    });
    bind_entry(&mut evidence, path, &after)?;
    bind_entry(&mut evidence, parent, next_parent)?;
    self.nodes = current;
    self.evidence = evidence;
    Ok(())
  }
}

fn bind_entry(digest: &mut Sha256, path: &Path, stamp: &Stamp) -> io::Result<()> {
  let name = path.as_os_str().as_bytes();
  digest.update((name.len() as u64).to_be_bytes());
  digest.update(name);
  let bytes = serde_json::to_vec(stamp)?;
  digest.update((bytes.len() as u64).to_be_bytes());
  digest.update(bytes);
  Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
  use std::fs;
  use std::os::unix::fs::{MetadataExt, PermissionsExt};

  use super::*;

  struct Fixture(PathBuf);
  impl Fixture {
    fn new() -> Self {
      let path = Path::new("/private/tmp").join(format!(
        "reap-cargo-inventory-{}",
        crate::journal::random_token().unwrap()
      ));
      fs::create_dir(&path).unwrap();
      assert_eq!(
        fs::metadata(&path).unwrap().dev(),
        fs::metadata("/System/Volumes/Data").unwrap().dev()
      );
      Self(path)
    }
  }
  impl Drop for Fixture {
    fn drop(&mut self) {
      fs::remove_dir_all(&self.0).unwrap();
    }
  }

  fn binding(inventory: &Inventory) -> String {
    let mut digest = Sha256::new();
    inventory.bind(&mut digest);
    format!("{:x}", digest.finalize())
  }

  #[test]
  fn freshness_refuses_changes_in_unselected_descendants_without_replacing_its_evidence() {
    for change in [
      "write",
      "replace",
      "missing",
      "add",
      "link",
      "mode",
      "directory",
    ] {
      let fixture = Fixture::new();
      let target = fixture.0.join("target");
      let nested = target.join("unselected/nested");
      fs::create_dir_all(&nested).unwrap();
      let kept = nested.join("kept");
      fs::write(&kept, b"kept bytes").unwrap();
      let outside = fixture.0.join("outside");
      fs::write(&outside, b"outside bytes").unwrap();
      let link = nested.join("link");
      std::os::unix::fs::symlink(&outside, &link).unwrap();
      fs::create_dir(nested.join("empty")).unwrap();
      let inventory = Inventory::capture(&target).unwrap();
      let original = binding(&inventory);
      inventory.fresh().unwrap();
      match change {
        "write" => fs::write(&kept, b"new! bytes").unwrap(),
        "replace" => {
          fs::rename(&kept, fixture.0.join("retained-original")).unwrap();
          fs::write(&kept, b"kept bytes").unwrap();
        }
        "missing" => fs::rename(&kept, fixture.0.join("retained-original")).unwrap(),
        "add" => fs::write(nested.join("new"), b"new work").unwrap(),
        "link" => {
          fs::remove_file(&link).unwrap();
          std::os::unix::fs::symlink(&kept, &link).unwrap();
        }
        "mode" => fs::set_permissions(&kept, fs::Permissions::from_mode(0o700)).unwrap(),
        "directory" => fs::remove_dir(nested.join("empty")).unwrap(),
        _ => unreachable!(),
      }
      assert!(inventory.fresh().is_err(), "{change}");
      assert_eq!(binding(&inventory), original, "{change}");
      assert_eq!(fs::read(&outside).unwrap(), b"outside bytes", "{change}");
    }
  }

  #[test]
  fn rejected_removal_prefix_cannot_rebind_unrelated_changes() {
    let fixture = Fixture::new();
    let target = fixture.0.join("target");
    fs::create_dir(&target).unwrap();
    let old = target.join("old");
    let kept = target.join("kept");
    fs::write(&old, b"obsolete output").unwrap();
    fs::write(&kept, b"kept output").unwrap();
    let mut inventory = Inventory::capture(&target).unwrap();
    let evidence = binding(&inventory);
    let original = File::open(&old).unwrap();
    fs::remove_file(&old).unwrap();
    fs::write(&kept, b"new output").unwrap();
    assert!(inventory.removed(&old, &original).is_err());
    assert_eq!(binding(&inventory), evidence);
    assert!(inventory.nodes.contains_key(&old));
    assert!(inventory.fresh().is_err());
    assert_eq!(fs::read(&kept).unwrap(), b"new output");
  }

  #[test]
  fn postorder_range_excludes_adjacent_component_prefixes() {
    let fixture = Fixture::new();
    let target = fixture.0.join("target");
    for name in ["a", "a-b", "a.hidden", "b"] {
      fs::create_dir_all(target.join(name)).unwrap();
      fs::write(target.join(name).join("output"), b"retained output").unwrap();
    }
    let inventory = Inventory::capture(&target).unwrap();
    assert_eq!(
      inventory.entries(&target.join("a")).unwrap(),
      vec![target.join("a/output"), target.join("a")]
    );
    inventory.fresh().unwrap();
  }

  #[test]
  fn exact_owned_prefix_preserves_hard_links_and_refuses_new_or_changed_inputs() {
    let fixture = Fixture::new();
    let target = fixture.0.join("target");
    fs::create_dir(&target).unwrap();
    let old = target.join("old");
    let kept = target.join("kept");
    fs::write(&old, b"retained bytes").unwrap();
    fs::hard_link(&old, &kept).unwrap();
    let outside = fixture.0.join("outside");
    fs::hard_link(&old, &outside).unwrap();
    let mut inventory = Inventory::capture(&target).unwrap();
    let before = binding(&inventory);
    inventory.fresh().unwrap();
    assert_eq!(binding(&inventory), before);
    let original = File::open(&old).unwrap();
    fs::remove_file(&old).unwrap();
    inventory.removed(&old, &original).unwrap();
    assert_ne!(binding(&inventory), before);
    inventory.fresh().unwrap();
    assert_eq!(fs::read(&outside).unwrap(), b"retained bytes");
    assert_eq!(fs::read(&kept).unwrap(), b"retained bytes");
    let fresh = target.join("fresh");
    fs::write(&fresh, b"new work").unwrap();
    assert!(inventory.fresh().is_err());
    assert_eq!(fs::read(&fresh).unwrap(), b"new work");
    fs::remove_file(&fresh).unwrap();
    assert!(inventory.fresh().is_err());
    fs::write(&kept, b"changed bytes").unwrap();
    assert!(inventory.fresh().is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"changed bytes");
  }

  #[test]
  fn postorder_keeps_policy_last_and_does_not_follow_child_links() {
    let fixture = Fixture::new();
    let target = fixture.0.join("target");
    fs::create_dir_all(target.join("selected/child")).unwrap();
    fs::write(target.join("selected/.reap"), b"owned policy").unwrap();
    fs::write(target.join("selected/child/data"), b"owned output").unwrap();
    let outside = fixture.0.join("outside");
    fs::write(&outside, b"survivor").unwrap();
    std::os::unix::fs::symlink(&outside, target.join("selected/link")).unwrap();
    let mut inventory = Inventory::capture(&target).unwrap();
    let paths = inventory.entries(&target.join("selected")).unwrap();
    assert_eq!(paths.last().unwrap(), &target.join("selected"));
    assert_eq!(paths[paths.len() - 2], target.join("selected/.reap"));
    assert!(!inventory.nodes.contains_key(&outside));
    for path in paths {
      inventory.fresh().unwrap();
      let held = crate::mutation::HeldUnlink::prepare(
        &path,
        &crate::launch::Resources::Bulk {
          physical_volumes: crate::volumes::resolve(&File::open(&target).unwrap()).unwrap(),
        },
        "a".repeat(64),
      )
      .unwrap();
      let original = held.source().try_clone().unwrap();
      inventory.source(&path, &original).unwrap();
      if fs::symlink_metadata(&path).unwrap().is_dir() {
        fs::remove_dir(&path).unwrap();
      } else {
        fs::remove_file(&path).unwrap();
      }
      inventory.removed(&path, &original).unwrap();
    }
    assert_eq!(fs::read(&outside).unwrap(), b"survivor");
    assert!(!target.join("selected").exists());
    inventory.fresh().unwrap();
  }

  #[test]
  #[ignore = "explicit Cargo inventory cost measurement"]
  fn inventory_cost_keeps_complete_validation_and_an_exact_removal_prefix() {
    use std::time::Instant;

    let fixture = Fixture::new();
    let target = fixture.0.join("target");
    for group in 0..64 {
      let directory = target.join(format!("group-{group:02}"));
      fs::create_dir_all(&directory).unwrap();
      for entry in 0..64 {
        fs::write(directory.join(format!("entry-{entry:02}")), b"build output").unwrap();
      }
    }
    let started = Instant::now();
    let mut inventory = Inventory::capture(&target).unwrap();
    let capture = started.elapsed();
    let started = Instant::now();
    for _ in 0..128 {
      std::hint::black_box(binding(&inventory));
    }
    let bind = started.elapsed();
    let started = Instant::now();
    for _ in 0..4 {
      inventory.fresh().unwrap();
    }
    let fresh = started.elapsed();
    let started = Instant::now();
    for entry in 0..16 {
      let path = target.join(format!("group-00/entry-{entry:02}"));
      inventory.fresh().unwrap();
      std::hint::black_box(binding(&inventory));
      let original = File::open(&path).unwrap();
      inventory.source(&path, &original).unwrap();
      fs::remove_file(&path).unwrap();
      inventory.removed(&path, &original).unwrap();
    }
    let prefix = started.elapsed();
    inventory.fresh().unwrap();
    assert_eq!(inventory.nodes.len(), 1 + 64 + 4096 - 16);
    assert_eq!(
      fs::read(target.join("group-63/entry-63")).unwrap(),
      b"build output"
    );
    eprintln!(
      "{}",
      serde_json::json!({
        "files":4096,"directories":65,"bindings":128,"fresh_checks":4,"removals":16,
        "capture_ms":capture.as_secs_f64()*1000.0,"bind_ms":bind.as_secs_f64()*1000.0,
        "fresh_ms":fresh.as_secs_f64()*1000.0,"prefix_ms":prefix.as_secs_f64()*1000.0,
      })
    );
  }
}
