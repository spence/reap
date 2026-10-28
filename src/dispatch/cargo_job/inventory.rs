//! Complete Cargo planning evidence and exact retirement of known owned removals.

use std::collections::BTreeMap;
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

/// A complete finite namespace, not permission to remove it. Child links remain leaves.
pub(super) struct Inventory {
  root: PathBuf,
  nodes: BTreeMap<PathBuf, Stamp>,
}

impl Inventory {
  /// Bind the complete current evidence without one aggregate serialization buffer.
  pub(super) fn bind(&self, digest: &mut Sha256) -> io::Result<()> {
    for (path, stamp) in &self.nodes {
      let name = path.as_os_str().as_bytes();
      digest.update((name.len() as u64).to_be_bytes());
      digest.update(name);
      let bytes = serde_json::to_vec(stamp)?;
      digest.update((bytes.len() as u64).to_be_bytes());
      digest.update(bytes);
    }
    Ok(())
  }

  pub(super) fn capture(root: &Path) -> io::Result<Self> {
    let directory = open_physical(root)?;
    let first = Stamp::read(&descriptor_stat(&directory)?);
    let mut nodes = BTreeMap::from([(root.to_path_buf(), first.clone())]);
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
      let held = open_relative(&directory, root, &path, first.device)?;
      let before = Stamp::read(&descriptor_stat(&held)?);
      if nodes.get(&path) != Some(&before) || !before.directory() {
        return Err(io::Error::other("Cargo directory changed during inventory"));
      }
      for_each_preview_name(&held, |name| {
        if nodes.len() >= MAX_ENTRIES {
          return Err(io::Error::other("Cargo inventory exceeds its entry bound"));
        }
        let name = Path::new(std::ffi::OsStr::from_bytes(name));
        let observed = stat_at(&held, name)?;
        let kind = observed.st_mode & libc::S_IFMT;
        if observed.st_dev as u64 != first.device
          || !matches!(kind, libc::S_IFDIR | libc::S_IFREG | libc::S_IFLNK)
        {
          return Err(io::Error::other(
            "Cargo inventory crosses a mount or has a special entry",
          ));
        }
        let child = path.join(name);
        if nodes
          .insert(child.clone(), Stamp::read(&observed))
          .is_some()
        {
          return Err(io::Error::other("Cargo inventory repeats an entry"));
        }
        if kind == libc::S_IFDIR {
          pending.push(child);
        }
        Ok(())
      })?;
      if Stamp::read(&descriptor_stat(&held)?) != before {
        return Err(io::Error::other("Cargo directory changed during inventory"));
      }
    }
    Ok(Self {
      root: root.into(),
      nodes,
    })
  }

  /// Refuse additions, replacements, content changes and incomplete namespace observations.
  /// This full comparison has linear cost in the captured target for each call.
  pub(super) fn fresh(&self) -> io::Result<()> {
    if Self::capture(&self.root)?.nodes != self.nodes {
      return Err(io::Error::other("Cargo planning input changed"));
    }
    Ok(())
  }

  /// Prepare a fixed postorder list. Policy entries follow all other children at their depth.
  pub(super) fn entries(&self, root: &Path) -> io::Result<Vec<PathBuf>> {
    if !self.nodes.contains_key(root) {
      return Err(io::Error::other(
        "Cargo selection is outside its captured inventory",
      ));
    }
    let mut paths: Vec<_> = self
      .nodes
      .keys()
      .filter(|path| path.starts_with(root))
      .cloned()
      .collect();
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
      return Err(io::Error::other("Cargo source differs from its selection"));
    }
    Ok(())
  }

  /// Retire exactly one completed unlink. Only its parent and matching hard-link metadata can
  /// change. A later mismatch stops work; it does not erase the already recorded unit effect.
  pub(super) fn removed(&mut self, path: &Path, original: &File) -> io::Result<()> {
    let before = self
      .nodes
      .get(path)
      .ok_or_else(|| io::Error::other("removed Cargo source was not inventoried"))?;
    let after = Stamp::read(&descriptor_stat(original)?);
    let mut expected_source = before.clone();
    expected_source.changed = after.changed;
    expected_source.links = if before.directory() {
      after.links
    } else {
      before
        .links
        .checked_sub(1)
        .ok_or_else(|| io::Error::other("Cargo source had no links"))?
    };
    if before.directory() {
      expected_source.size = after.size;
    }
    if after != expected_source {
      return Err(io::Error::other("Cargo source changed during its unlink"));
    }
    let current = Self::capture(&self.root)?;
    let parent = path
      .parent()
      .ok_or_else(|| io::Error::other("Cargo source has no parent"))?;
    let prior_parent = self
      .nodes
      .get(parent)
      .ok_or_else(|| io::Error::other("Cargo source parent was not inventoried"))?;
    let next_parent = current
      .nodes
      .get(parent)
      .filter(|next| prior_parent.same_object(next) && next.directory())
      .ok_or_else(|| io::Error::other("Cargo source parent changed identity"))?;
    let removed = before.clone();
    let mut expected = self.nodes.clone();
    expected.remove(path);
    expected.insert(parent.into(), next_parent.clone());
    for stamp in expected.values_mut() {
      if stamp.device == removed.device && stamp.inode == removed.inode {
        stamp.changed = after.changed;
        stamp.links = after.links;
      }
    }
    if expected != current.nodes {
      return Err(io::Error::other(
        "Cargo input differs from its exact owned removal prefix",
      ));
    }
    self.nodes = current.nodes;
    Ok(())
  }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
  use std::fs;
  use std::os::unix::fs::MetadataExt;

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
    inventory.fresh().unwrap();
    let original = File::open(&old).unwrap();
    fs::remove_file(&old).unwrap();
    inventory.removed(&old, &original).unwrap();
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
}
