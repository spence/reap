#!/usr/bin/env python3
"""Prepare an isolated diagnostic build. Do not replace the installed Reap binary."""

import hashlib
import json
import shutil
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "target" / "responsiveness-profile"

PROFILER = r'''
//! Diagnostic measurements for a source copy, not the installed cleanup path.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::json;

type Rows = BTreeMap<(String, String), (u64, f64)>;

struct State {
  started: Instant,
  rows: Rows,
  counts: BTreeMap<String, u64>,
  active: BTreeMap<u64, (String, String)>,
  sequence: u64,
  last_directory: String,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

pub struct Span {
  label: &'static str,
  path: String,
  started: Instant,
  id: Option<u64>,
}

pub fn init() {
  let args: Vec<String> = std::env::args().skip(1).collect();
  let allowed = ["files", "stores", "sweep", "list", "status", "inventory", "coverage", "check", "config", "doctor", "retire"];
  if !args.first().is_some_and(|arg| allowed.contains(&arg.as_str()))
    || args.iter().any(|arg| ["--apply", "--init", "--bind", "--now"].contains(&arg.as_str()))
    || std::env::var("REAP_PROFILE_READ_ONLY").as_deref() != Ok("1")
  {
    eprintln!("diagnostic build: an explicit read-only command and REAP_PROFILE_READ_ONLY=1 are required");
    std::process::exit(2);
  }
  STATE.set(Mutex::new(State {
    started: Instant::now(), rows: BTreeMap::new(), counts: BTreeMap::new(),
    active: BTreeMap::new(), sequence: 0, last_directory: String::new(),
  })).ok().expect("one diagnostic initialization");
  emit("started");
  std::thread::spawn(|| loop {
    std::thread::sleep(Duration::from_secs(5));
    emit("progress");
  });
}

pub fn span(label: &'static str, path: &Path) -> Span {
  let directory = label.ends_with(".directory");
  let mut state = STATE.get().expect("initialized").lock().unwrap();
  let text = path.to_string_lossy().into_owned();
  let path = if directory {
    state.last_directory = text;
    let parts: Vec<_> = path.components().collect();
    let offset = parts.iter().position(|p| p.as_os_str() == "projects")
      .or_else(|| parts.iter().position(|p| p.as_os_str() == "src"));
    parts.iter().take(offset.map_or(parts.len(), |n| n + 3))
      .map(|p| p.as_os_str()).collect::<std::path::PathBuf>()
      .to_string_lossy().into_owned()
  } else { text };
  let id = if directory { None } else {
    state.sequence += 1;
    let id = state.sequence;
    state.active.insert(id, (label.to_string(), path.clone()));
    Some(id)
  };
  Span { label, path, started: Instant::now(), id }
}

impl Drop for Span {
  fn drop(&mut self) {
    let mut state = STATE.get().expect("initialized").lock().unwrap();
    let row = state.rows.entry((self.label.to_string(), self.path.clone())).or_default();
    row.0 += 1;
    row.1 += self.started.elapsed().as_secs_f64();
    if let Some(id) = self.id { state.active.remove(&id); }
  }
}

pub fn count(label: &str) {
  let mut state = STATE.get().expect("initialized").lock().unwrap();
  *state.counts.entry(label.to_string()).or_default() += 1;
}

fn cpu(who: i32) -> serde_json::Value {
  let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
  // SAFETY: getrusage initializes this structure on success.
  if unsafe { libc::getrusage(who, usage.as_mut_ptr()) } != 0 { return json!(null); }
  // SAFETY: The preceding call succeeded.
  let usage = unsafe { usage.assume_init() };
  json!({"user": usage.ru_utime.tv_sec as f64 + usage.ru_utime.tv_usec as f64 / 1e6,
    "system": usage.ru_stime.tv_sec as f64 + usage.ru_stime.tv_usec as f64 / 1e6})
}

pub fn emit(kind: &str) {
  let state = STATE.get().expect("initialized").lock().unwrap();
  let rows: Vec<_> = state.rows.iter().map(|((label, path), (calls, seconds))|
    json!({"label": label, "path": path, "calls": calls, "seconds": seconds})).collect();
  eprintln!("REAP_PROFILE {}", json!({"kind": kind, "elapsed": state.started.elapsed().as_secs_f64(),
    "self_cpu": cpu(libc::RUSAGE_SELF), "children_cpu": cpu(libc::RUSAGE_CHILDREN),
    "rows": rows, "counts": state.counts, "active": state.active,
    "last_directory": state.last_directory}));
}
'''


def replace_once(text, before, after):
    """Refuse a source revision that does not match this diagnostic patch."""
    before, after = "\n" + before, "\n" + after
    if text.count(before) != 1:
        raise RuntimeError(f"Expected one source match: {before!r}")
    return text.replace(before, after, 1)


def prepare():
    """Copy tracked crate inputs and add measurements without a cleanup policy change."""
    DESTINATION.mkdir(parents=True, exist_ok=False)
    digests = {}
    files = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", *sorted((ROOT / "src").glob("*.rs"))]
    for source in files:
        relative = source.relative_to(ROOT)
        target = DESTINATION / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(source.read_bytes())
        digests[str(relative)] = hashlib.sha256(source.read_bytes()).hexdigest()

    def patch(filename, changes):
        path = DESTINATION / "src" / filename
        text = path.read_text()
        for before, after in changes:
            text = replace_once(text, before, after)
        path.write_text(text)

    patch("main.rs", [
        ("fn main() {", "mod responsiveness_profile;\n\nfn main() {\n  responsiveness_profile::init();"),
        ("  exit(code);", '  responsiveness_profile::emit("finished");\n  exit(code);'),
    ])
    prefix = "crate::responsiveness_profile::"
    patch("files.rs", [
        ('fn git_tracked(file: &Path) -> bool {', f'fn git_tracked(file: &Path) -> bool {{\n  let _profile = {prefix}span("files.git_tracked", file);'),
        ('pub fn scan(root: &Path, now: i64, grace_secs: i64) -> Result<Scan, String> {', f'pub fn scan(root: &Path, now: i64, grace_secs: i64) -> Result<Scan, String> {{\n  let _profile = {prefix}span("files.scan", root);'),
        ('  while let Some(dir) = stack.pop() {', f'  while let Some(dir) = stack.pop() {{\n    let _directory = {prefix}span("files.discovery.directory", &dir);'),
        ('    for e in rd.flatten() {', f'    for e in rd.flatten() {{\n      {prefix}count("files.discovery.entries");'),
        ('fn unit_stats(path: &Path) -> Option<(f64, u64, Option<PathBuf>)> {', f'fn unit_stats(path: &Path) -> Option<(f64, u64, Option<PathBuf>)> {{\n  let _profile = {prefix}span("files.unit_stats", path);'),
        ('    let ev = evaluate(&scan.view, now as i64);', f'    let ev = {{ let _profile = {prefix}span("files.evaluate", &root); evaluate(&scan.view, now as i64) }};'),
        ('  let p = &r.path;', f'  let p = &r.path;\n  let _profile = {prefix}span("files.blocked", p);'),
        ('fn git_unrecoverable(dir: &Path) -> Option<String> {', f'fn git_unrecoverable(dir: &Path) -> Option<String> {{\n  let _profile = {prefix}span("files.git_unrecoverable", dir);'),
    ])
    patch("util.rs", [
        ('  let mut st = TreeStats {', f'  let _profile = {prefix}span("tree_stats", root);\n  let mut st = TreeStats {{'),
        ('  while let Some((dir, under_git)) = stack.pop() {', f'  while let Some((dir, under_git)) = stack.pop() {{\n    let _directory = {prefix}span("tree.directory", &dir);'),
        ('    for entry in rd.flatten() {', f'    for entry in rd.flatten() {{\n      {prefix}count("tree.entries");'),
        ('pub fn git_capture_bytes(dir: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {', f'pub fn git_capture_bytes(dir: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {{\n  let _profile = {prefix}span("git_capture", dir);'),
        ('pub fn lock_state(state: &Path) -> io::Result<File> {', f'pub fn lock_state(state: &Path) -> io::Result<File> {{\n  let _profile = {prefix}span("state_lock", state);'),
    ])
    patch("discover.rs", [
        ('pub fn discover_targets(roots: &[PathBuf], exclude: &[String]) -> DiscoverResult {', f'pub fn discover_targets(roots: &[PathBuf], exclude: &[String]) -> DiscoverResult {{\n  let _profile = {prefix}span("discover.targets", Path::new("all-roots"));'),
        ('pub fn discover_manifests(roots: &[PathBuf], exclude: &[String]) -> (Vec<PathBuf>, Vec<String>) {', f'pub fn discover_manifests(roots: &[PathBuf], exclude: &[String]) -> (Vec<PathBuf>, Vec<String>) {{\n  let _profile = {prefix}span("discover.manifests", Path::new("all-roots"));'),
    ])
    discovery = DESTINATION / "src" / "discover.rs"
    text = discovery.read_text()
    for before, after in [
        ('\n  while let Some(dir) = stack.pop() {', f'\n  while let Some(dir) = stack.pop() {{\n    let _directory = {prefix}span("discovery.directory", &dir);'),
        ('\n    for entry in rd.flatten() {', f'\n    for entry in rd.flatten() {{\n      {prefix}count("discovery.entries");'),
    ]:
        if text.count(before) != 2:
            raise RuntimeError(f"Expected two discovery loops: {before!r}")
        text = text.replace(before, after)
    discovery.write_text(text)
    patch("stores.rs", [
        ('  let state = if armed {', f'  let _profile = {prefix}span("stores.plan_one", &dir);\n  let state = if armed {{'),
        ('fn store_tree_stats(root: &Path, store_dev: u64) -> Result<StoreTreeStats, String> {', f'fn store_tree_stats(root: &Path, store_dev: u64) -> Result<StoreTreeStats, String> {{\n  let _profile = {prefix}span("stores.tree_stats", root);'),
    ])
    patch("plan.rs", [
        ('pub fn plan_project(manifest: &Manifest, now: f64, quick: bool) -> Result<Plan, Protected> {', f'pub fn plan_project(manifest: &Manifest, now: f64, quick: bool) -> Result<Plan, Protected> {{\n  let _profile = {prefix}span("cargo.plan_project", &manifest.target_dir());'),
    ])
    (DESTINATION / "src" / "responsiveness_profile.rs").write_text(PROFILER)
    receipt = {
        "source_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "source_sha256": digests,
        "instrumenter_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "note": "Nested span times overlap. Counts and timing include diagnostic overhead. No deletion-policy changes.",
    }
    (DESTINATION / "source-receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(DESTINATION)


if __name__ == "__main__":
    prepare()
