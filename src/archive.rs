//! Immutable native conversation custody, independent of job records and source homes.
//!
//! A native adapter supplies a closed, immutable snapshot and ordered NDJSON inventories.
//! The coordinator supplies held reservations and fresh destination/restore authorization.
//! This module never launches a native client, copies credentials, or releases a source.

use std::ffi::{CStr, CString, OsStr};
use std::fs::{File, Metadata};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Maximum serialized manifest size, independent of permanent payload capacity.
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
/// Maximum serialized NDJSON record size. Larger records refuse the whole capture.
pub const MAX_RECORD_BYTES: usize = 256 * 1024;
const BUFFER_BYTES: usize = 64 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DEPTH: usize = 64;
const PERMANENT: &[u8] = b"{\"version\":2,\"keep\":{\"reason\":\"permanent conversation custody\"},\"seal\":true,\"role\":\"permanent_archive\"}\n";
const PRIVATE_KEEP: &[u8] = b"{\"version\":2,\"keep\":{\"reason\":\"private native retrieval; no live promotion\"},\"seal\":true}\n";

/// The two native profiles. Versions and closure are verified by the real adapter.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
  Codex,
  Claude,
}

impl Provider {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Codex => "codex",
      Self::Claude => "claude",
    }
  }
}

/// Exact archive-local slot identity. Native conversation IDs are not capture IDs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureKey {
  pub source_host: String,
  pub provider: Provider,
  pub archive_id: String,
}

/// Native location and historical ownership evidence, including explicit uncertainty.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attribution {
  pub execution_origin: Option<String>,
  pub account: Option<String>,
  pub project: Option<String>,
  pub evidence: String,
}

/// A source alias and the archive-local canonical payload that replaces its dependency.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Alias {
  pub original_path: String,
  pub canonical_original_path: String,
  pub archived_path: String,
}

/// A profile-established component boundary, not an inferred timestamp.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
  pub component: String,
  pub before: String,
  pub after: String,
}

/// Stable input to capture replay. Inventory digests bind every native payload unit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePlan {
  pub key: CaptureKey,
  pub dataset_id: String,
  pub store_id: String,
  pub profile_version: String,
  pub client_version: String,
  pub policy_digest: String,
  pub original_homes: Vec<String>,
  pub original_paths: Vec<String>,
  pub aliases: Vec<Alias>,
  pub boundaries: Vec<Boundary>,
  pub attribution: Vec<Attribution>,
  pub files_sha256: String,
  pub conversations_sha256: String,
}

/// SQLite online-backup evidence. The native verifier must attest this actual operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SqliteSnapshot {
  pub step_result: i32,
  pub finish_result: i32,
  pub integrity_result: String,
  pub boundary: String,
}

/// Native file/link units. Links are stored as links, never traversed during copying.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EntryKind {
  Directory,
  File {
    bytes: u64,
    sha256: String,
    sqlite: Option<SqliteSnapshot>,
  },
  Symlink {
    target: String,
  },
}

/// One file inventory record. Paths use canonical byte-preserving percent encoding.
/// Native permission and time metadata are evidence; archive files remain private/read-only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
  pub path: String,
  pub source: FileSource,
  pub kind: EntryKind,
  pub mode: u32,
  pub modified_seconds: i64,
  pub modified_nanoseconds: i64,
}

/// Original native location and profile classification for one payload unit.
/// Only generated container directories may omit an original absolute path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileSource {
  pub original_path: Option<String>,
  pub component: String,
  pub native_role: String,
}

/// A native dependency, optionally with its provider-specific database record locator.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
  pub path: String,
  pub record_locator: Option<String>,
}

/// A group member with native identity, lineage, dependencies, and origin evidence.
/// The adapter validates native parent/child closure and record locators, not Reap row IDs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Conversation {
  pub group_id: String,
  pub native_id: String,
  pub parents: Vec<String>,
  pub children: Vec<String>,
  /// Profile-required members of this same group, distinct from uncertain lineage attribution.
  pub required_members: Vec<String>,
  pub dependencies: Vec<Dependency>,
  pub attribution: Vec<Attribution>,
  pub unresolved_dependencies: Vec<String>,
}

/// Job-owned immutable snapshot. Its root contains the exact `native/` inventory tree.
/// SQLite files must come from completed online backups, not live file copies.
pub struct PreparedSnapshot {
  pub root: PathBuf,
  pub files_inventory: PathBuf,
  pub conversations_inventory: PathBuf,
}

/// Native verification result, retained in the immutable manifest.
/// This evidence does not grant source release or prove a native restore acceptance gate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeVerification {
  pub profile_version: String,
  pub client_version: String,
  pub boundary_sha256: String,
  pub verifier: String,
  pub evidence: String,
  pub restore_limits: Vec<String>,
}

/// Mandatory integration with the real Codex or Claude snapshot/verification procedure.
/// Implementations must fail on unsupported versions, incomplete closure, or unknown boundaries.
/// No implementation or success fallback is provided by this module.
pub trait NativeVerifier {
  /// Validate the copied native group, all dependencies, and actual SQLite DONE evidence.
  fn verify(&self, plan: &CapturePlan, staged: &File) -> io::Result<NativeVerification>;
  /// Revalidate original source identity, aliases, closure, and generation before publication.
  fn source_unchanged(&self, plan: &CapturePlan) -> io::Result<()>;
}

/// A pinned destination binding. Device/inode identify the held live root, not durable adoption.
/// The coordinator independently verifies host, volume UUID, configured scope, and root binding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationBinding {
  pub path: PathBuf,
  pub archive_host: String,
  pub volume_uuid: String,
  pub device: u64,
  pub inode: u64,
}

/// Explicit reservation under the project's approved allocation. No numeric default exists.
/// The full byte allowance must be reserved before any payload write.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capacity {
  pub reservation_id: String,
  pub reserved_bytes: u64,
}

/// Coordinator boundary for fresh identity, complete conflict sets, capacity, and isolation.
/// Validation runs before writes and immediately before completion. It must not wait on conflicts.
pub trait CustodyAuthority {
  /// Verify the actual volume UUID/host/root, full reservations, and available reserved capacity.
  fn destination(
    &self,
    binding: &DestinationBinding,
    root: &File,
    capacity: &Capacity,
  ) -> io::Result<()>;
  /// Verify a new effectively kept private target, outside live homes and collector discovery.
  fn private_restore(&self, parent: &File, target: &Path, capacity: &Capacity) -> io::Result<()>;
}

/// Readable immutable commit metadata. Large inventories remain separate NDJSON streams.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
  pub format_version: u32,
  pub plan: CapturePlan,
  pub archive_host: String,
  pub destination_volume_uuid: String,
  pub native_verification: NativeVerification,
  pub file_entries: u64,
  pub conversation_entries: u64,
  pub payload_bytes: u64,
}

/// Archive-local completion evidence, independent of the seven-day job-result window.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
  pub format_version: u32,
  pub key: CaptureKey,
  pub manifest_sha256: String,
  pub payload_root: String,
  pub conversations_sha256: String,
}

/// A bound archive root. Opening never creates a missing destination or mountpoint.
pub struct Archive {
  root: File,
  binding: DestinationBinding,
}

impl Archive {
  /// Open an existing protected root through no-follow directory handles.
  pub fn open(binding: DestinationBinding) -> io::Result<Self> {
    component(&binding.archive_host)?;
    nonempty(&binding.volume_uuid)?;
    let root = open_absolute_dir(&binding.path)?;
    bound_root(&root, &binding)?;
    permanent(&root)?;
    Ok(Self { root, binding })
  }

  /// Stage, verify, synchronize, and exclusively publish a native capture.
  /// Failures retain unpublished protected staging; they never mutate or prune source data.
  pub fn capture(
    &self,
    plan: &CapturePlan,
    snapshot: &PreparedSnapshot,
    capacity: &Capacity,
    authority: &dyn CustodyAuthority,
    native: &dyn NativeVerifier,
  ) -> io::Result<Receipt> {
    validate_plan(plan)?;
    validate_capacity(capacity)?;
    self.authorize(capacity, authority)?;
    let parent = self.slot_parent(&plan.key, true)?;
    match open_dir(&parent, plan.key.archive_id.as_bytes()) {
      Ok(_) => {
        let existing = self.get(&plan.key)?;
        if existing.manifest.plan != *plan {
          return Err(invalid("capture ID conflicts with committed content"));
        }
        return Ok(existing.receipt);
      }
      Err(e) if e.kind() == io::ErrorKind::NotFound => {}
      Err(e) => return Err(e),
    }

    let source = open_absolute_dir(&snapshot.root)?;
    if overlaps(&source, &self.root)? {
      return Err(invalid("snapshot and permanent archive overlap"));
    }
    let incoming_name = format!(".incoming-{}", plan.key.archive_id);
    let incoming = make_dir(&parent, incoming_name.as_bytes(), false)?;
    let mut budget = Budget {
      remaining: capacity.reserved_bytes,
    };
    write_new(&incoming, b".reap", PERMANENT, &mut budget)?;
    write_json(
      &incoming,
      b"incoming.owner.json",
      &(plan.key.clone(), capacity),
      &mut budget,
    )?;
    durable_sync(&incoming)?;
    durable_sync(&parent)?;

    let (file_entries, payload_bytes) = copy_inventory(
      &source,
      &incoming,
      &snapshot.files_inventory,
      &plan.files_sha256,
      &mut budget,
    )?;
    verify_tree(&source, file_entries)?;
    let conversation_entries = copy_conversations(
      &incoming,
      &snapshot.conversations_inventory,
      &plan.conversations_sha256,
      &mut budget,
    )?;
    validate_graph(&incoming)?;
    validate_aliases(plan, &incoming)?;
    verify_tree(&incoming, file_entries)?;
    let native_verification = native.verify(plan, &incoming)?;
    validate_native(plan, &native_verification)?;
    let manifest = Manifest {
      format_version: 1,
      plan: plan.clone(),
      archive_host: self.binding.archive_host.clone(),
      destination_volume_uuid: self.binding.volume_uuid.clone(),
      native_verification,
      file_entries,
      conversation_entries,
      payload_bytes,
    };
    let manifest_bytes = json_bytes(&manifest, MAX_MANIFEST_BYTES)?;
    write_new(&incoming, b"manifest.json", &manifest_bytes, &mut budget)?;
    let receipt = Receipt {
      format_version: 1,
      key: plan.key.clone(),
      manifest_sha256: digest(&manifest_bytes),
      payload_root: plan.files_sha256.clone(),
      conversations_sha256: plan.conversations_sha256.clone(),
    };
    write_json(&incoming, b"capture.complete", &receipt, &mut budget)?;

    // verify the published representation, then flush all data and directories before rename.
    validate_capture(&incoming, &plan.key)?;
    seal_directories(&incoming, 0)?;
    sync_tree(&incoming)?;
    native.source_unchanged(plan)?;
    self.authorize(capacity, authority)?;
    same_child(&parent, incoming_name.as_bytes(), &incoming)?;
    exclusive_rename(
      &parent,
      incoming_name.as_bytes(),
      plan.key.archive_id.as_bytes(),
    )?;
    durable_sync(&parent)?;
    Ok(receipt)
  }

  /// Validate a complete capture from archive-local files alone.
  pub fn get(&self, key: &CaptureKey) -> io::Result<Capture> {
    validate_key(key)?;
    bound_root(&self.root, &self.binding)?;
    permanent(&self.root)?;
    let parent = self.slot_parent(key, false)?;
    let directory = open_dir(&parent, key.archive_id.as_bytes())?;
    let (manifest, receipt) = validate_capture(&directory, key)?;
    sync_tree(&directory)?;
    durable_sync(&parent)?;
    Ok(Capture {
      directory,
      manifest,
      receipt,
      archive_root: self.root.try_clone()?,
    })
  }

  /// Stream complete captures. Unpublished incoming slots are not completion evidence.
  /// A malformed or corrupt published slot fails the operation rather than disappearing.
  pub fn list(
    &self,
    mut visit: impl FnMut(&Manifest, &Receipt) -> io::Result<()>,
  ) -> io::Result<()> {
    for_each_name(&self.root, |host| {
      if host == b".reap" {
        return Ok(());
      }
      let host_name = std::str::from_utf8(host).map_err(|_| invalid("invalid source host"))?;
      component(host_name)?;
      let host_dir = open_dir(&self.root, host)?;
      for_each_name(&host_dir, |provider_name| {
        let provider = match provider_name {
          b"codex" => Provider::Codex,
          b"claude" => Provider::Claude,
          _ => return Err(invalid("unknown archive provider")),
        };
        let provider_dir = open_dir(&host_dir, provider_name)?;
        for_each_name(&provider_dir, |id| {
          if id.starts_with(b".incoming-") {
            return Ok(());
          }
          let archive_id = std::str::from_utf8(id).map_err(|_| invalid("invalid capture ID"))?;
          let key = CaptureKey {
            source_host: host_name.to_owned(),
            provider,
            archive_id: archive_id.to_owned(),
          };
          let capture = self.get(&key)?;
          visit(&capture.manifest, &capture.receipt)
        })
      })
    })
  }

  /// Stream matches by original store/native ID. Distinct captures remain distinct results.
  pub fn find(
    &self,
    provider: Provider,
    store_id: &str,
    native_id: &str,
    mut visit: impl FnMut(&Receipt, &Conversation) -> io::Result<()>,
  ) -> io::Result<()> {
    self.list(|manifest, receipt| {
      if manifest.plan.key.provider == provider && manifest.plan.store_id == store_id {
        self.get(&manifest.plan.key)?.conversations(|entry| {
          if entry.native_id == native_id {
            visit(receipt, entry)?;
          }
          Ok(())
        })?;
      }
      Ok(())
    })
  }

  fn authorize(&self, capacity: &Capacity, authority: &dyn CustodyAuthority) -> io::Result<()> {
    bound_root(&self.root, &self.binding)?;
    let current = open_absolute_dir(&self.binding.path)?;
    if identity(&current)? != identity(&self.root)? {
      return Err(invalid("destination replaced"));
    }
    permanent(&self.root)?;
    authority.destination(&self.binding, &self.root, capacity)
  }

  fn slot_parent(&self, key: &CaptureKey, create: bool) -> io::Result<File> {
    validate_key(key)?;
    let host = namespace_dir(&self.root, key.source_host.as_bytes(), create)?;
    let provider = namespace_dir(&host, key.provider.as_str().as_bytes(), create)?;
    if identity(&host)?.0 != self.binding.device || identity(&provider)?.0 != self.binding.device {
      return Err(invalid(
        "archive namespace crosses the bound destination volume",
      ));
    }
    durable_sync(&self.root)?;
    durable_sync(&host)?;
    Ok(provider)
  }
}

/// A fully validated capture held through an archive directory handle.
pub struct Capture {
  directory: File,
  archive_root: File,
  pub manifest: Manifest,
  pub receipt: Receipt,
}

impl Capture {
  /// Expose a read-only directory handle for a real version-specific native verifier.
  pub fn directory(&self) -> &File {
    &self.directory
  }

  /// Stream conversation evidence without a local index or original source home.
  pub fn conversations(
    &self,
    mut visit: impl FnMut(&Conversation) -> io::Result<()>,
  ) -> io::Result<()> {
    let mut hash = Sha256::new();
    let mut previous = String::new();
    let count =
      read_records::<Conversation>(&self.directory, b"conversations.ndjson", |entry, line| {
        validate_conversation(&self.directory, &entry, &mut previous)?;
        hash.update(line);
        visit(&entry)
      })?;
    if count != self.manifest.conversation_entries
      || hex_digest(hash) != self.receipt.conversations_sha256
    {
      return Err(invalid("conversation inventory changed"));
    }
    Ok(())
  }

  /// Create a new kept 0700 private copy. Existing targets are never merged or overwritten.
  /// The authority must exclude live homes and AIMeter discovery roots before this call.
  /// Failure leaves a kept incomplete copy; no native program runs implicitly.
  pub fn restore_private(
    &self,
    parent_path: &Path,
    name: &str,
    capacity: &Capacity,
    authority: &dyn CustodyAuthority,
  ) -> io::Result<PathBuf> {
    component(name)?;
    validate_capacity(capacity)?;
    validate_capture(&self.directory, &self.manifest.plan.key)?;
    let parent = open_absolute_dir(parent_path)?;
    if overlaps(&parent, &self.archive_root)? {
      return Err(invalid("restore target overlaps permanent archive"));
    }
    let path = parent_path.join(name);
    authority.private_restore(&parent, &path, capacity)?;
    let destination = make_dir(&parent, name.as_bytes(), false)?;
    let mut budget = Budget {
      remaining: capacity.reserved_bytes,
    };
    write_new(&destination, b".reap", PRIVATE_KEEP, &mut budget)?;
    durable_sync(&destination)?;
    durable_sync(&parent)?;
    let (count, bytes) =
      copy_records_between(&self.directory, &destination, &self.directory, &mut budget)?;
    if count != self.manifest.file_entries || bytes != self.manifest.payload_bytes {
      return Err(invalid("restore inventory changed"));
    }
    validate_aliases(&self.manifest.plan, &destination)?;
    verify_tree(&destination, count)?;
    read_records::<FileEntry>(&self.directory, b"files.ndjson", |entry, _| {
      verify_entry(&destination, &entry)
    })?;
    for (name, expected) in [
      (
        b"manifest.json".as_slice(),
        self.receipt.manifest_sha256.as_str(),
      ),
      (b"files.ndjson", self.receipt.payload_root.as_str()),
      (
        b"conversations.ndjson",
        self.receipt.conversations_sha256.as_str(),
      ),
    ] {
      copy_metadata(&self.directory, &destination, name, expected, &mut budget)?;
    }
    write_json(
      &destination,
      b"restoration.json",
      &self.receipt,
      &mut budget,
    )?;
    sync_tree(&destination)?;
    authority.private_restore(&parent, &path, capacity)?;
    same_child(&parent, name.as_bytes(), &destination)?;
    durable_sync(&parent)?;
    Ok(path)
  }
}

/// Encode native path bytes without loss, including non-UTF-8 filenames.
/// Absolute source locations may use this encoding in provenance, but payload paths must be relative.
pub fn encode_path(path: &Path) -> String {
  encode_bytes(path.as_os_str().as_bytes())
}

/// Compute the boundary digest that the native verifier must return.
pub fn boundary_digest(plan: &CapturePlan) -> io::Result<String> {
  Ok(digest(&json_bytes(&plan.boundaries, MAX_MANIFEST_BYTES)?))
}

/// Read a single allowlisted snapshot unit without following source links.
/// The adapter supplies SQLite evidence only for a completed bounded backup.
pub fn inspect_file(
  root: &File,
  path: &str,
  source: FileSource,
  sqlite: Option<SqliteSnapshot>,
) -> io::Result<FileEntry> {
  let raw = payload_path(path)?;
  let (parent, name) = path_parent(root, &raw)?;
  let metadata = lstat(&parent, &name)?;
  let kind = match metadata.st_mode & libc::S_IFMT {
    libc::S_IFDIR => EntryKind::Directory,
    libc::S_IFREG => {
      let mut file = open_file(&parent, &name)?;
      let (bytes, sha256, is_sqlite) = hash_file(&mut file, None)?;
      validate_sqlite(is_sqlite, &sqlite)?;
      EntryKind::File {
        bytes,
        sha256,
        sqlite,
      }
    }
    libc::S_IFLNK => {
      let target = read_link(&parent, &name)?;
      resolve_link(&raw, &target)?;
      EntryKind::Symlink {
        target: encode_bytes(&target),
      }
    }
    _ => return Err(invalid("unsupported native file type")),
  };
  let (seconds, nanoseconds) = stat_mtime(&metadata);
  Ok(FileEntry {
    path: path.to_owned(),
    source,
    kind,
    mode: metadata.st_mode as u32 & 0o7777,
    modified_seconds: seconds,
    modified_nanoseconds: nanoseconds,
  })
}

fn copy_inventory(
  source: &File,
  destination: &File,
  input: &Path,
  expected: &str,
  budget: &mut Budget,
) -> io::Result<(u64, u64)> {
  let file = open_absolute_file(input)?;
  let before = signature(&file.metadata()?);
  let mut output = new_file(destination, b"files.ndjson")?;
  let mut hash = Sha256::new();
  let mut previous = String::new();
  let mut bytes = 0u64;
  let count = records::<FileEntry>(file.try_clone()?, |entry, line| {
    ordered(&mut previous, &entry.path)?;
    budget.take(line.len() as u64)?;
    bytes = add(
      bytes,
      copy_entry(source, destination, &entry, budget, true)?,
    )?;
    output.write_all(line)?;
    hash.update(line);
    Ok(())
  })?;
  if count == 0 || hex_digest(hash) != expected || signature(&file.metadata()?) != before {
    return Err(invalid("incomplete or changed file inventory"));
  }
  durable_sync(&output)?;
  readonly(&output)?;
  Ok((count, bytes))
}

fn copy_conversations(
  destination: &File,
  input: &Path,
  expected: &str,
  budget: &mut Budget,
) -> io::Result<u64> {
  let file = open_absolute_file(input)?;
  let before = signature(&file.metadata()?);
  let mut output = new_file(destination, b"conversations.ndjson")?;
  let mut hash = Sha256::new();
  let mut previous = String::new();
  let count = records::<Conversation>(file.try_clone()?, |entry, line| {
    validate_conversation(destination, &entry, &mut previous)?;
    budget.take(line.len() as u64)?;
    output.write_all(line)?;
    hash.update(line);
    Ok(())
  })?;
  if count == 0 || hex_digest(hash) != expected || signature(&file.metadata()?) != before {
    return Err(invalid("incomplete or changed conversation inventory"));
  }
  durable_sync(&output)?;
  readonly(&output)?;
  Ok(count)
}

fn copy_records_between(
  source: &File,
  destination: &File,
  inventory: &File,
  budget: &mut Budget,
) -> io::Result<(u64, u64)> {
  let mut bytes = 0u64;
  let mut previous = String::new();
  let count = read_records::<FileEntry>(inventory, b"files.ndjson", |entry, _| {
    ordered(&mut previous, &entry.path)?;
    bytes = add(
      bytes,
      copy_entry(source, destination, &entry, budget, false)?,
    )?;
    Ok(())
  })?;
  Ok((count, bytes))
}

fn copy_metadata(
  source: &File,
  destination: &File,
  name: &[u8],
  expected: &str,
  budget: &mut Budget,
) -> io::Result<()> {
  let mut input = open_file(source, name)?;
  let mut output = new_file(destination, name)?;
  let before = signature(&input.metadata()?);
  budget.take(before.2)?;
  let (copied, hash, _) = hash_file(&mut input, Some((&mut output, before.2)))?;
  if copied != before.2 || hash != expected || signature(&input.metadata()?) != before {
    return Err(invalid("archive metadata changed during restore"));
  }
  durable_sync(&output)?;
  readonly(&output)
}

fn copy_entry(
  source: &File,
  destination: &File,
  entry: &FileEntry,
  budget: &mut Budget,
  native_metadata: bool,
) -> io::Result<u64> {
  validate_entry(entry)?;
  let path = payload_path(&entry.path)?;
  let (source_parent, name) = path_parent(source, &path)?;
  let (destination_parent, _) = path_parent(destination, &path)?;
  let before = lstat(&source_parent, &name)?;
  if native_metadata
    && (stat_mtime(&before) != (entry.modified_seconds, entry.modified_nanoseconds)
      || before.st_mode as u32 & 0o7777 != entry.mode)
  {
    return Err(invalid("source metadata changed since inventory"));
  }
  match &entry.kind {
    EntryKind::Directory => {
      if before.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(invalid("source type changed"));
      }
      let source_dir = open_dir(&source_parent, &name)?;
      let destination_dir = make_dir(&destination_parent, &name, false)?;
      source_dir.metadata()?;
      durable_sync(&destination_dir)?;
    }
    EntryKind::File {
      bytes,
      sha256,
      sqlite,
    } => {
      if before.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(invalid("source type changed"));
      }
      budget.take(*bytes)?;
      let mut input = open_file(&source_parent, &name)?;
      let signature_before = signature(&input.metadata()?);
      if signature_before.2 != *bytes {
        return Err(invalid("source length changed"));
      }
      let mut output = new_file(&destination_parent, &name)?;
      let (actual_bytes, actual_hash, is_sqlite) =
        hash_file(&mut input, Some((&mut output, *bytes)))?;
      validate_sqlite(is_sqlite, sqlite)?;
      if actual_bytes != *bytes
        || actual_hash != *sha256
        || signature(&input.metadata()?) != signature_before
      {
        return Err(invalid("source content changed or corrupt"));
      }
      durable_sync(&output)?;
      readonly(&output)?;
    }
    EntryKind::Symlink { target } => {
      if before.st_mode & libc::S_IFMT != libc::S_IFLNK {
        return Err(invalid("source type changed"));
      }
      let raw_target = decode_bytes(target)?;
      resolve_link(&path, &raw_target)?;
      if read_link(&source_parent, &name)? != raw_target {
        return Err(invalid("source link changed"));
      }
      budget.take(raw_target.len() as u64)?;
      symlink_at(&destination_parent, &name, &raw_target)?;
    }
  }
  let after = lstat(&source_parent, &name)?;
  if stat_signature(&before) != stat_signature(&after) {
    return Err(invalid("source unit changed"));
  }
  durable_sync(&destination_parent)?;
  Ok(match &entry.kind {
    EntryKind::File { bytes, .. } => *bytes,
    _ => 0,
  })
}

fn validate_capture(directory: &File, key: &CaptureKey) -> io::Result<(Manifest, Receipt)> {
  permanent(directory)?;
  let manifest_bytes = read_small(directory, b"manifest.json", MAX_MANIFEST_BYTES)?;
  let manifest: Manifest = parse(&manifest_bytes)?;
  let receipt: Receipt = parse(&read_small(
    directory,
    b"capture.complete",
    MAX_MANIFEST_BYTES,
  )?)?;
  validate_plan(&manifest.plan)?;
  validate_native(&manifest.plan, &manifest.native_verification)?;
  component(&manifest.archive_host)?;
  nonempty(&manifest.destination_volume_uuid)?;
  if manifest.format_version != 1
    || receipt.format_version != 1
    || manifest.plan.key != *key
    || receipt.key != *key
    || receipt.manifest_sha256 != digest(&manifest_bytes)
    || receipt.payload_root != manifest.plan.files_sha256
    || receipt.conversations_sha256 != manifest.plan.conversations_sha256
  {
    return Err(invalid("invalid completion evidence"));
  }
  let mut previous = String::new();
  let mut payload = 0u64;
  let mut files_hash = Sha256::new();
  let file_count = read_records::<FileEntry>(directory, b"files.ndjson", |entry, line| {
    validate_entry(&entry)?;
    validate_component(&manifest.plan, &entry)?;
    ordered(&mut previous, &entry.path)?;
    verify_entry(directory, &entry)?;
    if let EntryKind::File { bytes, .. } = entry.kind {
      payload = add(payload, bytes)?;
    }
    files_hash.update(line);
    Ok(())
  })?;
  let mut previous = String::new();
  let mut conversations_hash = Sha256::new();
  let conversation_count =
    read_records::<Conversation>(directory, b"conversations.ndjson", |entry, line| {
      validate_conversation(directory, &entry, &mut previous)?;
      conversations_hash.update(line);
      Ok(())
    })?;
  if file_count == 0
    || conversation_count == 0
    || file_count != manifest.file_entries
    || conversation_count != manifest.conversation_entries
    || payload != manifest.payload_bytes
    || hex_digest(files_hash) != manifest.plan.files_sha256
    || hex_digest(conversations_hash) != manifest.plan.conversations_sha256
  {
    return Err(invalid("capture inventory is incomplete or corrupt"));
  }
  validate_aliases(&manifest.plan, directory)?;
  validate_graph(directory)?;
  verify_tree(directory, file_count)?;
  for_each_name(directory, |name| {
    if ![
      b".reap".as_slice(),
      b"native",
      b"files.ndjson",
      b"conversations.ndjson",
      b"manifest.json",
      b"capture.complete",
      b"incoming.owner.json",
    ]
    .contains(&name)
    {
      return Err(invalid("unexpected capture component"));
    }
    Ok(())
  })?;
  Ok((manifest, receipt))
}

fn verify_entry(root: &File, entry: &FileEntry) -> io::Result<()> {
  let path = payload_path(&entry.path)?;
  let (parent, name) = path_parent(root, &path)?;
  let metadata = lstat(&parent, &name)?;
  match &entry.kind {
    EntryKind::Directory => {
      open_dir(&parent, &name)?;
    }
    EntryKind::File {
      bytes,
      sha256,
      sqlite,
    } => {
      let mut file = open_file(&parent, &name)?;
      if file.metadata()?.nlink() != 1 {
        return Err(invalid("archive file has external hard links"));
      }
      let before = signature(&file.metadata()?);
      let (actual_bytes, actual_hash, is_sqlite) = hash_file(&mut file, None)?;
      if actual_bytes != *bytes || actual_hash != *sha256 || signature(&file.metadata()?) != before
      {
        return Err(invalid("archive payload is corrupt or changed"));
      }
      validate_sqlite(is_sqlite, sqlite)?;
    }
    EntryKind::Symlink { target } => {
      if metadata.st_mode & libc::S_IFMT != libc::S_IFLNK
        || read_link(&parent, &name)? != decode_bytes(target)?
      {
        return Err(invalid("archive link is corrupt"));
      }
      let resolved = resolve_link(&path, &decode_bytes(target)?)?;
      let (target_parent, target_name) = path_parent(root, &resolved)?;
      let target_stat = lstat(&target_parent, &target_name)?;
      if ![libc::S_IFDIR, libc::S_IFREG].contains(&(target_stat.st_mode & libc::S_IFMT)) {
        return Err(invalid(
          "link target is not an archive-local file or directory",
        ));
      }
    }
  }
  Ok(())
}

fn validate_plan(plan: &CapturePlan) -> io::Result<()> {
  json_bytes(plan, MAX_MANIFEST_BYTES)?;
  validate_key(&plan.key)?;
  for value in [
    &plan.dataset_id,
    &plan.store_id,
    &plan.profile_version,
    &plan.client_version,
  ] {
    nonempty(value)?;
  }
  for value in [
    &plan.policy_digest,
    &plan.files_sha256,
    &plan.conversations_sha256,
  ] {
    sha256(value)?;
  }
  if plan.original_homes.is_empty() || plan.original_paths.is_empty() || plan.boundaries.is_empty()
  {
    return Err(invalid("missing native source or boundary evidence"));
  }
  for path in plan.original_homes.iter().chain(&plan.original_paths) {
    if !Path::new(OsStr::from_bytes(&decode_bytes(path)?)).is_absolute() {
      return Err(invalid("original source location is not absolute"));
    }
  }
  let mut previous = String::new();
  for boundary in &plan.boundaries {
    nonempty(&boundary.component)?;
    ordered(&mut previous, &boundary.component)?;
    nonempty(&boundary.before)?;
    if boundary.before != boundary.after {
      return Err(invalid("native boundary changed"));
    }
  }
  for origin in &plan.attribution {
    nonempty(&origin.evidence)?;
  }
  Ok(())
}

fn validate_native(plan: &CapturePlan, native: &NativeVerification) -> io::Result<()> {
  if native.profile_version != plan.profile_version
    || native.client_version != plan.client_version
    || native.boundary_sha256 != boundary_digest(plan)?
  {
    return Err(invalid(
      "native verification does not bind this capture boundary",
    ));
  }
  nonempty(&native.verifier)?;
  nonempty(&native.evidence)
}

fn validate_conversation(
  root: &File,
  entry: &Conversation,
  previous: &mut String,
) -> io::Result<()> {
  nonempty(&entry.group_id)?;
  nonempty(&entry.native_id)?;
  // the separator cannot appear in either ID, so the order is unambiguous and unique.
  ordered(
    previous,
    &format!("{}\0{}", entry.group_id, entry.native_id),
  )?;
  if !entry.unresolved_dependencies.is_empty() || entry.dependencies.is_empty() {
    return Err(invalid("native dependency closure is incomplete"));
  }
  for id in entry
    .parents
    .iter()
    .chain(&entry.children)
    .chain(&entry.required_members)
  {
    nonempty(id)?;
  }
  for dependency in &entry.dependencies {
    let raw = payload_path(&dependency.path)?;
    let (parent, name) = path_parent(root, &raw)?;
    let metadata = lstat(&parent, &name)?;
    if ![libc::S_IFREG, libc::S_IFDIR].contains(&(metadata.st_mode & libc::S_IFMT)) {
      return Err(invalid("native dependency is missing or unsafe"));
    }
    if let Some(locator) = &dependency.record_locator {
      nonempty(locator)?;
      let mut file = open_file(&parent, &name)?;
      let mut header = [0u8; 16];
      file.read_exact(&mut header)?;
      if &header != b"SQLite format 3\0" {
        return Err(invalid("record locator does not name SQLite"));
      }
    }
  }
  for origin in &entry.attribution {
    nonempty(&origin.evidence)?;
  }
  Ok(())
}

fn validate_graph(root: &File) -> io::Result<()> {
  read_records::<Conversation>(root, b"conversations.ndjson", |entry, _| {
    for id in &entry.required_members {
      if id == &entry.native_id || !conversation_exists(root, &entry.group_id, id)? {
        return Err(invalid("native group omits a referenced parent or child"));
      }
    }
    Ok(())
  })?;
  Ok(())
}

// ordered NDJSON supports bounded binary search without a catalog, sort runs, or a full ID set.
fn conversation_exists(root: &File, group: &str, native_id: &str) -> io::Result<bool> {
  let mut file = BufReader::with_capacity(BUFFER_BYTES, open_file(root, b"conversations.ndjson")?);
  let mut low = 0;
  let mut high = file.get_ref().metadata()?.len();
  let mut line = Vec::with_capacity(MAX_RECORD_BYTES);
  while low < high {
    let middle = low + (high - low) / 2;
    file.seek(SeekFrom::Start(middle.saturating_sub(1)))?;
    if middle != 0 {
      let mut previous = [0];
      file.read_exact(&mut previous)?;
      if previous[0] != b'\n' {
        line.clear();
        (&mut file)
          .take((MAX_RECORD_BYTES + 1) as u64)
          .read_until(b'\n', &mut line)?;
        if line.len() > MAX_RECORD_BYTES {
          return Err(invalid("oversized conversation record"));
        }
      }
    }
    let start = file.stream_position()?;
    if start >= high {
      high = middle;
      continue;
    }
    line.clear();
    (&mut file)
      .take((MAX_RECORD_BYTES + 1) as u64)
      .read_until(b'\n', &mut line)?;
    if line.len() > MAX_RECORD_BYTES || line.last() != Some(&b'\n') {
      return Err(invalid("incomplete conversation record"));
    }
    let entry: Conversation = parse(&line)?;
    match (entry.group_id.as_str(), entry.native_id.as_str()).cmp(&(group, native_id)) {
      std::cmp::Ordering::Equal => return Ok(true),
      std::cmp::Ordering::Less => low = file.stream_position()?,
      std::cmp::Ordering::Greater => high = middle,
    }
  }
  Ok(false)
}

fn validate_aliases(plan: &CapturePlan, root: &File) -> io::Result<()> {
  for alias in &plan.aliases {
    for original in [&alias.original_path, &alias.canonical_original_path] {
      if !Path::new(OsStr::from_bytes(&decode_bytes(original)?)).is_absolute() {
        return Err(invalid("alias source is not absolute"));
      }
    }
    let (parent, name) = path_parent(root, &payload_path(&alias.archived_path)?)?;
    let metadata = lstat(&parent, &name)?;
    if ![libc::S_IFDIR, libc::S_IFREG].contains(&(metadata.st_mode & libc::S_IFMT)) {
      return Err(invalid(
        "alias mapping has no archive-local canonical payload",
      ));
    }
  }
  Ok(())
}

fn validate_entry(entry: &FileEntry) -> io::Result<()> {
  payload_path(&entry.path)?;
  nonempty(&entry.source.component)?;
  nonempty(&entry.source.native_role)?;
  if let Some(original) = &entry.source.original_path {
    if !Path::new(OsStr::from_bytes(&decode_bytes(original)?)).is_absolute() {
      return Err(invalid("native file origin is not absolute"));
    }
  } else if !matches!(entry.kind, EntryKind::Directory) {
    return Err(invalid("native payload unit lacks its original path"));
  }
  if entry.mode & !0o7777 != 0 || !(0..1_000_000_000).contains(&entry.modified_nanoseconds) {
    return Err(invalid("invalid native file metadata"));
  }
  match &entry.kind {
    EntryKind::File {
      sha256: hash,
      sqlite,
      ..
    } => {
      sha256(hash)?;
      validate_sqlite(sqlite.is_some(), sqlite)?;
    }
    EntryKind::Symlink { target } => {
      resolve_link(&payload_path(&entry.path)?, &decode_bytes(target)?)?;
    }
    EntryKind::Directory => {}
  }
  Ok(())
}

fn validate_component(plan: &CapturePlan, entry: &FileEntry) -> io::Result<()> {
  let boundary = plan
    .boundaries
    .iter()
    .find(|b| b.component == entry.source.component)
    .ok_or_else(|| invalid("native unit lacks a component boundary"))?;
  if let EntryKind::File {
    sqlite: Some(snapshot),
    ..
  } = &entry.kind
  {
    if snapshot.boundary != boundary.after {
      return Err(invalid(
        "SQLite evidence names a different component boundary",
      ));
    }
  }
  Ok(())
}

fn validate_sqlite(is_sqlite: bool, snapshot: &Option<SqliteSnapshot>) -> io::Result<()> {
  if is_sqlite != snapshot.is_some() {
    return Err(invalid("SQLite requires completed snapshot evidence"));
  }
  if let Some(snapshot) = snapshot {
    if snapshot.step_result != 101
      || snapshot.finish_result != 0
      || snapshot.integrity_result != "ok"
    {
      return Err(invalid(
        "SQLite backup did not reach DONE and successful finalization/integrity",
      ));
    }
    nonempty(&snapshot.boundary)?;
  }
  Ok(())
}

fn validate_key(key: &CaptureKey) -> io::Result<()> {
  component(&key.source_host)?;
  component(&key.archive_id)
}

fn validate_capacity(capacity: &Capacity) -> io::Result<()> {
  nonempty(&capacity.reservation_id)?;
  if capacity.reserved_bytes == 0 {
    return Err(invalid("capacity reservation is empty"));
  }
  Ok(())
}

struct Budget {
  remaining: u64,
}
impl Budget {
  fn take(&mut self, bytes: u64) -> io::Result<()> {
    self.remaining = self
      .remaining
      .checked_sub(bytes)
      .ok_or_else(|| invalid("reserved capacity exhausted"))?;
    Ok(())
  }
}

fn records<T: DeserializeOwned>(
  file: File,
  mut visit: impl FnMut(T, &[u8]) -> io::Result<()>,
) -> io::Result<u64> {
  let before = signature(&file.metadata()?);
  let mut reader = BufReader::with_capacity(BUFFER_BYTES, file.try_clone()?.take(before.2));
  let mut line = Vec::with_capacity(MAX_RECORD_BYTES);
  let mut count = 0u64;
  let mut consumed = 0u64;
  loop {
    line.clear();
    let mut limited = (&mut reader).take((MAX_RECORD_BYTES + 1) as u64);
    let length = limited.read_until(b'\n', &mut line)?;
    if length == 0 {
      break;
    }
    if length > MAX_RECORD_BYTES || line.last() != Some(&b'\n') || line == b"\n" {
      return Err(invalid("oversized or incomplete NDJSON record"));
    }
    visit(parse(&line)?, &line)?;
    count = add(count, 1)?;
    consumed = add(consumed, length as u64)?;
  }
  if consumed != before.2 || signature(&file.metadata()?) != before {
    return Err(invalid("inventory changed during traversal"));
  }
  Ok(count)
}

fn read_records<T: DeserializeOwned>(
  root: &File,
  name: &[u8],
  visit: impl FnMut(T, &[u8]) -> io::Result<()>,
) -> io::Result<u64> {
  records(open_file(root, name)?, visit)
}

fn hash_file(
  file: &mut File,
  mut copy: Option<(&mut File, u64)>,
) -> io::Result<(u64, String, bool)> {
  let mut buffer = [0u8; BUFFER_BYTES];
  let mut hash = Sha256::new();
  let mut bytes = 0u64;
  let mut first = [0u8; 16];
  let mut first_len = 0;
  let limit = copy
    .as_ref()
    .map_or(file.metadata()?.len(), |(_, limit)| *limit);
  loop {
    let n = file.read(&mut buffer)?;
    if n == 0 {
      break;
    }
    let head = (16 - first_len).min(n);
    first[first_len..first_len + head].copy_from_slice(&buffer[..head]);
    first_len += head;
    bytes = add(bytes, n as u64)?;
    if bytes > limit {
      return Err(invalid("file grew beyond its captured boundary"));
    }
    if let Some((output, limit)) = copy.as_mut() {
      if bytes > *limit {
        return Err(invalid("source grew beyond reserved payload"));
      }
      output.write_all(&buffer[..n])?;
    }
    hash.update(&buffer[..n]);
  }
  Ok((
    bytes,
    hex_digest(hash),
    first_len == 16 && &first == b"SQLite format 3\0",
  ))
}

fn verify_tree(root: &File, expected: u64) -> io::Result<()> {
  let native = open_dir(root, b"native")?;
  if identity(&native)?.0 != identity(root)?.0 {
    return Err(invalid("native tree crosses its staging volume"));
  }
  let actual = add(1, walk(&native, 1, false)?)?;
  if actual != expected {
    return Err(invalid("payload inventory omits or adds units"));
  }
  Ok(())
}

fn sync_tree(root: &File) -> io::Result<()> {
  walk(root, 0, true).map(|_| ())
}

fn seal_directories(root: &File, depth: usize) -> io::Result<()> {
  if depth > MAX_DEPTH {
    return Err(invalid("native directory depth exceeds bound"));
  }
  for_each_name(root, |name| {
    if lstat(root, name)?.st_mode & libc::S_IFMT == libc::S_IFDIR {
      seal_directories(&open_dir(root, name)?, depth + 1)?;
    }
    Ok(())
  })?;
  // SAFETY: root is an exclusively created incoming directory; no source directory reaches this path.
  if unsafe { libc::fchmod(root.as_raw_fd(), 0o500) } != 0 {
    return Err(io::Error::last_os_error());
  }
  durable_sync(root)
}

fn walk(root: &File, depth: usize, sync: bool) -> io::Result<u64> {
  if depth > MAX_DEPTH {
    return Err(invalid("native directory depth exceeds bound"));
  }
  let mut count = 0u64;
  for_each_name(root, |name| {
    let metadata = lstat(root, name)?;
    if metadata.st_dev as u64 != identity(root)?.0 {
      return Err(invalid("payload traversal crosses its staging volume"));
    }
    count = add(count, 1)?;
    match metadata.st_mode & libc::S_IFMT {
      libc::S_IFDIR => {
        count = add(count, walk(&open_dir(root, name)?, depth + 1, sync)?)?;
      }
      libc::S_IFREG => {
        if sync {
          durable_sync(&open_file(root, name)?)?;
        }
      }
      libc::S_IFLNK => {}
      _ => return Err(invalid("unsupported payload unit")),
    }
    Ok(())
  })?;
  if sync {
    durable_sync(&root)?;
  }
  Ok(count)
}

fn permanent(directory: &File) -> io::Result<()> {
  let value: serde_json::Value = parse(&read_small(directory, b".reap", 64 * 1024)?)?;
  if value["version"] != 2
    || value["role"] != "permanent_archive"
    || value["seal"] != true
    || value["keep"]["reason"]
      .as_str()
      .is_none_or(|s| s.trim().is_empty())
    || !value["expires"].is_null()
    || !value["work"].is_null()
    || value["scratch"] == true
    || value
      .get("children")
      .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
  {
    return Err(invalid("destination lacks permanent sealed KEEP"));
  }
  Ok(())
}

fn bound_root(root: &File, binding: &DestinationBinding) -> io::Result<()> {
  if identity(root)? != (binding.device, binding.inode) {
    return Err(invalid("destination root identity changed"));
  }
  Ok(())
}

fn payload_path(encoded: &str) -> io::Result<Vec<u8>> {
  let raw = decode_bytes(encoded)?;
  let path = Path::new(OsStr::from_bytes(&raw));
  if raw.len() > MAX_PATH_BYTES
    || raw.contains(&0)
    || raw.is_empty()
    || path.is_absolute()
    || raw
      .split(|b| *b == b'/')
      .any(|part| part.is_empty() || part == b"." || part == b"..")
    || path.components().count() > MAX_DEPTH
    || !(raw == b"native" || raw.starts_with(b"native/"))
  {
    return Err(invalid("unsafe native payload path"));
  }
  Ok(raw)
}

fn resolve_link(path: &[u8], target: &[u8]) -> io::Result<Vec<u8>> {
  if target.is_empty() || target.len() > MAX_PATH_BYTES || target[0] == b'/' || target.contains(&0)
  {
    return Err(invalid("unsafe native link target"));
  }
  let mut parts: Vec<&[u8]> = path.split(|b| *b == b'/').collect();
  parts.pop();
  for part in target.split(|b| *b == b'/') {
    match part {
      b"" | b"." => {}
      b".." => {
        if parts.len() <= 1 {
          return Err(invalid("native link escapes payload"));
        }
        parts.pop();
      }
      _ => parts.push(part),
    }
  }
  let result = parts.join(&b'/');
  payload_path(&encode_bytes(&result))?;
  Ok(result)
}

fn encode_bytes(bytes: &[u8]) -> String {
  let mut output = String::new();
  for &b in bytes {
    if (0x20..0x7f).contains(&b) && b != b'%' {
      output.push(b as char);
    } else {
      output.push_str(&format!("%{b:02x}"));
    }
  }
  output
}

fn decode_bytes(encoded: &str) -> io::Result<Vec<u8>> {
  if encoded.len() > MAX_PATH_BYTES * 3 {
    return Err(invalid("encoded path exceeds bound"));
  }
  let mut raw = Vec::with_capacity(encoded.len());
  let mut bytes = encoded.bytes();
  while let Some(b) = bytes.next() {
    if b == b'%' {
      let a = bytes.next().and_then(|v| (v as char).to_digit(16));
      let b = bytes.next().and_then(|v| (v as char).to_digit(16));
      let (Some(a), Some(b)) = (a, b) else {
        return Err(invalid("invalid encoded filename"));
      };
      raw.push((a * 16 + b) as u8);
    } else {
      raw.push(b);
    }
  }
  if encode_bytes(&raw) != encoded || raw.contains(&0) {
    return Err(invalid("noncanonical encoded filename"));
  }
  Ok(raw)
}

fn component(value: &str) -> io::Result<()> {
  if value.is_empty()
    || value.len() > 180
    || value.starts_with('.')
    || !value
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
  {
    return Err(invalid("invalid archive path component"));
  }
  Ok(())
}

fn nonempty(value: &str) -> io::Result<()> {
  if value.trim().is_empty() || value.contains('\0') {
    return Err(invalid("missing native evidence"));
  }
  Ok(())
}

fn sha256(value: &str) -> io::Result<()> {
  if value.len() != 64
    || !value
      .bytes()
      .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
  {
    return Err(invalid("invalid SHA-256 digest"));
  }
  Ok(())
}

fn ordered(previous: &mut String, next: &str) -> io::Result<()> {
  if !previous.is_empty() && previous.as_str() >= next {
    return Err(invalid("inventory is not strictly ordered"));
  }
  *previous = next.to_owned();
  Ok(())
}

fn add(a: u64, b: u64) -> io::Result<u64> {
  a.checked_add(b)
    .ok_or_else(|| invalid("inventory size overflow"))
}
fn invalid(message: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, message)
}
fn parse<T: DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
  serde_json::from_slice(bytes).map_err(io::Error::other)
}
fn digest(bytes: &[u8]) -> String {
  format!("{:x}", Sha256::digest(bytes))
}
fn hex_digest(hash: Sha256) -> String {
  format!("{:x}", hash.finalize())
}

fn json_bytes(value: &impl Serialize, limit: usize) -> io::Result<Vec<u8>> {
  // the bounded writer stops serde before a large caller-owned structure can allocate a second copy.
  struct Limited {
    bytes: Vec<u8>,
    limit: usize,
  }
  impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
      if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
        return Err(invalid("JSON exceeds bound"));
      }
      self.bytes.extend_from_slice(bytes);
      Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
      Ok(())
    }
  }
  let mut writer = Limited {
    bytes: Vec::new(),
    limit,
  };
  serde_json::to_writer(&mut writer, value).map_err(io::Error::other)?;
  Ok(writer.bytes)
}

fn read_small(parent: &File, name: &[u8], limit: usize) -> io::Result<Vec<u8>> {
  let mut bytes = Vec::new();
  open_file(parent, name)?
    .take(limit as u64 + 1)
    .read_to_end(&mut bytes)?;
  if bytes.len() > limit {
    return Err(invalid("archive metadata exceeds bound"));
  }
  Ok(bytes)
}

fn write_json(
  parent: &File,
  name: &[u8],
  value: &impl Serialize,
  budget: &mut Budget,
) -> io::Result<()> {
  write_new(
    parent,
    name,
    &json_bytes(value, MAX_MANIFEST_BYTES)?,
    budget,
  )
}

fn write_new(parent: &File, name: &[u8], bytes: &[u8], budget: &mut Budget) -> io::Result<()> {
  budget.take(bytes.len() as u64)?;
  let mut file = new_file(parent, name)?;
  file.write_all(bytes)?;
  durable_sync(&file)?;
  readonly(&file)
}

fn cstring(bytes: &[u8]) -> io::Result<CString> {
  CString::new(bytes).map_err(|_| invalid("path contains NUL"))
}

fn open_at(parent: &File, name: &[u8], flags: i32) -> io::Result<File> {
  let name = cstring(name)?;
  // SAFETY: parent is held, name is terminated, and the returned descriptor has exclusive ownership.
  let fd = unsafe {
    libc::openat(
      parent.as_raw_fd(),
      name.as_ptr(),
      flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
      0o600,
    )
  };
  if fd < 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: openat returned a fresh owned descriptor.
  Ok(unsafe { File::from_raw_fd(fd) })
}

fn open_dir(parent: &File, name: &[u8]) -> io::Result<File> {
  open_at(parent, name, libc::O_RDONLY | libc::O_DIRECTORY)
}
fn open_file(parent: &File, name: &[u8]) -> io::Result<File> {
  let file = open_at(parent, name, libc::O_RDONLY)?;
  if !file.metadata()?.is_file() {
    return Err(invalid("archive component is not a regular file"));
  }
  Ok(file)
}
fn new_file(parent: &File, name: &[u8]) -> io::Result<File> {
  open_at(parent, name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)
}

fn open_absolute_dir(path: &Path) -> io::Result<File> {
  if !path.is_absolute() {
    return Err(invalid("directory binding must be absolute"));
  }
  let mut directory = File::open("/")?;
  for part in path.components() {
    match part {
      Component::RootDir => {}
      Component::Normal(name) => directory = open_dir(&directory, name.as_bytes())?,
      _ => return Err(invalid("unsafe directory binding")),
    }
  }
  Ok(directory)
}

fn open_absolute_file(path: &Path) -> io::Result<File> {
  let parent = path
    .parent()
    .ok_or_else(|| invalid("inventory has no parent"))?;
  let name = path
    .file_name()
    .ok_or_else(|| invalid("inventory has no filename"))?;
  open_file(&open_absolute_dir(parent)?, name.as_bytes())
}

fn path_parent(root: &File, raw: &[u8]) -> io::Result<(File, Vec<u8>)> {
  let mut parts = raw.split(|b| *b == b'/').peekable();
  let mut directory = root.try_clone()?;
  let device = identity(root)?.0;
  while let Some(part) = parts.next() {
    if parts.peek().is_none() {
      return Ok((directory, part.to_vec()));
    }
    directory = open_dir(&directory, part)?;
    if identity(&directory)?.0 != device {
      return Err(invalid("native path crosses a staging volume"));
    }
  }
  Err(invalid("empty payload path"))
}

fn make_dir(parent: &File, name: &[u8], open_existing: bool) -> io::Result<File> {
  if open_existing {
    return open_dir(parent, name);
  }
  let terminated = cstring(name)?;
  // SAFETY: valid held parent and terminated direct-child name; mkdirat never replaces a child.
  if unsafe { libc::mkdirat(parent.as_raw_fd(), terminated.as_ptr(), 0o700) } != 0 {
    return Err(io::Error::last_os_error());
  }
  open_dir(parent, name)
}

fn namespace_dir(parent: &File, name: &[u8], create: bool) -> io::Result<File> {
  match open_dir(parent, name) {
    Ok(directory) => Ok(directory),
    Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
      match make_dir(parent, name, false) {
        Ok(directory) => Ok(directory),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => open_dir(parent, name),
        Err(error) => Err(error),
      }
    }
    Err(error) => Err(error),
  }
}

fn identity(file: &File) -> io::Result<(u64, u64)> {
  let meta = file.metadata()?;
  Ok((meta.dev(), meta.ino()))
}

fn same_child(parent: &File, name: &[u8], expected: &File) -> io::Result<()> {
  if identity(&open_dir(parent, name)?)? != identity(expected)? {
    return Err(invalid("incoming slot replaced"));
  }
  Ok(())
}

fn ancestor(child: &File, other: &File) -> io::Result<bool> {
  let target = identity(other)?;
  let mut current = child.try_clone()?;
  for _ in 0..MAX_PATH_BYTES {
    let id = identity(&current)?;
    if id == target {
      return Ok(true);
    }
    let parent = open_dir(&current, b"..")?;
    if identity(&parent)? == id {
      return Ok(false);
    }
    current = parent;
  }
  Err(invalid("directory ancestry exceeds bound"))
}
fn overlaps(a: &File, b: &File) -> io::Result<bool> {
  Ok(ancestor(a, b)? || ancestor(b, a)?)
}

#[cfg(test)]
thread_local! {
  static SYNC_FAULT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn durable_sync(file: &File) -> io::Result<()> {
  #[cfg(test)]
  SYNC_FAULT.with(|fault| {
    if let Some(remaining) = fault.get() {
      if remaining == 0 {
        return Err(io::Error::from_raw_os_error(libc::EIO));
      }
      fault.set(Some(remaining - 1));
    }
    Ok(())
  })?;
  file.sync_all()
}

fn readonly(file: &File) -> io::Result<()> {
  // SAFETY: the held descriptor belongs to an exclusively created archive file.
  if unsafe { libc::fchmod(file.as_raw_fd(), 0o400) } != 0 {
    return Err(io::Error::last_os_error());
  }
  durable_sync(&file)
}

fn lstat(parent: &File, name: &[u8]) -> io::Result<libc::stat> {
  let name = cstring(name)?;
  let mut result = std::mem::MaybeUninit::uninit();
  // SAFETY: valid output buffer, held parent, and terminated name; links remain unresolved.
  if unsafe {
    libc::fstatat(
      parent.as_raw_fd(),
      name.as_ptr(),
      result.as_mut_ptr(),
      libc::AT_SYMLINK_NOFOLLOW,
    )
  } != 0
  {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: successful fstatat initialized the structure.
  Ok(unsafe { result.assume_init() })
}

fn read_link(parent: &File, name: &[u8]) -> io::Result<Vec<u8>> {
  let name = cstring(name)?;
  let mut bytes = vec![0u8; MAX_PATH_BYTES + 1];
  // SAFETY: bytes is writable for its length; parent and terminated name remain valid.
  let n = unsafe {
    libc::readlinkat(
      parent.as_raw_fd(),
      name.as_ptr(),
      bytes.as_mut_ptr().cast(),
      bytes.len(),
    )
  };
  if n < 0 {
    return Err(io::Error::last_os_error());
  }
  if n as usize > MAX_PATH_BYTES {
    return Err(invalid("link exceeds path bound"));
  }
  bytes.truncate(n as usize);
  Ok(bytes)
}

fn symlink_at(parent: &File, name: &[u8], target: &[u8]) -> io::Result<()> {
  let name = cstring(name)?;
  let target = cstring(target)?;
  // SAFETY: both strings are terminated; symlinkat creates one new child without following target.
  if unsafe { libc::symlinkat(target.as_ptr(), parent.as_raw_fd(), name.as_ptr()) } != 0 {
    return Err(io::Error::last_os_error());
  }
  Ok(())
}

fn exclusive_rename(parent: &File, from: &[u8], to: &[u8]) -> io::Result<()> {
  let from = cstring(from)?;
  let to = cstring(to)?;
  #[cfg(target_os = "macos")]
  // SAFETY: both names are terminated direct children on the same held volume; RENAME_EXCL forbids replacement.
  let status = unsafe {
    libc::renameatx_np(
      parent.as_raw_fd(),
      from.as_ptr(),
      parent.as_raw_fd(),
      to.as_ptr(),
      libc::RENAME_EXCL,
    )
  };
  #[cfg(target_os = "linux")]
  // SAFETY: both names are terminated direct children on the same held volume; NOREPLACE forbids replacement.
  let status = unsafe {
    libc::renameat2(
      parent.as_raw_fd(),
      from.as_ptr(),
      parent.as_raw_fd(),
      to.as_ptr(),
      libc::RENAME_NOREPLACE,
    )
  };
  #[cfg(not(any(target_os = "linux", target_os = "macos")))]
  let status = {
    return Err(io::Error::new(
      io::ErrorKind::Unsupported,
      "exclusive archive publish unavailable",
    ));
  };
  if status != 0 {
    return Err(io::Error::last_os_error());
  }
  Ok(())
}

fn for_each_name(parent: &File, mut visit: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
  let independent = open_dir(parent, b".")?;
  use std::os::fd::IntoRawFd;
  let fd = independent.into_raw_fd();
  // SAFETY: fdopendir takes the fresh descriptor; this stream does not share another reader's offset.
  let stream = unsafe { libc::fdopendir(fd) };
  if stream.is_null() {
    let error = io::Error::last_os_error();
    // SAFETY: failed fdopendir did not take ownership of the fresh descriptor.
    unsafe {
      libc::close(fd);
    }
    return Err(error);
  }
  struct Directory(*mut libc::DIR);
  impl Drop for Directory {
    fn drop(&mut self) {
      // SAFETY: this guard uniquely owns the live DIR stream.
      unsafe {
        libc::closedir(self.0);
      }
    }
  }
  let stream = Directory(stream);
  loop {
    set_errno_zero();
    // SAFETY: stream is live; the returned entry is consumed before the next readdir call.
    let entry = unsafe { libc::readdir(stream.0) };
    if entry.is_null() {
      let error = io::Error::last_os_error();
      if error.raw_os_error() != Some(0) {
        return Err(error);
      }
      return Ok(());
    }
    // SAFETY: readdir returned a native terminated filename in its live entry buffer.
    let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
    if name != b"." && name != b".." {
      visit(name)?;
    }
  }
}

fn set_errno_zero() {
  #[cfg(target_os = "macos")]
  // SAFETY: __error returns this thread's valid errno pointer.
  unsafe {
    *libc::__error() = 0;
  }
  #[cfg(target_os = "linux")]
  // SAFETY: __errno_location returns this thread's valid errno pointer.
  unsafe {
    *libc::__errno_location() = 0;
  }
}

fn signature(meta: &Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
  (
    meta.dev(),
    meta.ino(),
    meta.len(),
    meta.mtime(),
    meta.mtime_nsec(),
    meta.ctime(),
    meta.ctime_nsec(),
  )
}
fn stat_signature(meta: &libc::stat) -> (u64, u64, i64, i64, i64, i64, i64) {
  let (mtime, mtime_ns) = stat_mtime(meta);
  #[cfg(target_os = "macos")]
  let (ctime, ctime_ns) = (meta.st_ctime, meta.st_ctime_nsec);
  #[cfg(target_os = "linux")]
  let (ctime, ctime_ns) = (meta.st_ctime, meta.st_ctime_nsec);
  (
    meta.st_dev as u64,
    meta.st_ino as u64,
    meta.st_size,
    mtime,
    mtime_ns,
    ctime,
    ctime_ns,
  )
}
fn stat_mtime(meta: &libc::stat) -> (i64, i64) {
  (meta.st_mtime, meta.st_mtime_nsec)
}

#[cfg(test)]
mod tests;
