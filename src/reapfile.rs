//! `.reap` lifetime declarations (`docs/specs/reap-file.md`, SPEC-REAP-FILE).
//!
//! A `.reap` file inside a directory declares that directory's own lifetime and optional rules
//! for its direct children. Evaluation is a pure function over a [`View`] of a tree, so the spec's
//! fixture corpus and a real directory scan run the same rules. Anything that cannot be read with
//! certainty protects its subtree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const FILE_NAME: &str = ".reap";

/// One parsed `.reap` file (C5–C7, C12, C13).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decl {
  pub version: u32,
  #[serde(default)]
  pub expires: Option<String>,
  #[serde(default)]
  pub keep: Option<Keep>,
  #[serde(default)]
  pub seal: Option<bool>,
  #[serde(default)]
  pub children: Option<Vec<Rule>>,
  #[serde(default)]
  pub disposition: Option<String>,
  #[serde(default)]
  pub owner: Option<String>,
  #[serde(default)]
  pub purpose: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keep {
  pub reason: String,
  #[serde(default)]
  pub review_after: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
  pub pattern: String,
  #[serde(default)]
  pub keep_newest: Option<usize>,
  #[serde(default)]
  pub max_age_days: Option<f64>,
  #[serde(default)]
  pub max_count: Option<usize>,
  #[serde(default)]
  pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
  Quarantine,
  Delete,
}

impl Decl {
  pub fn parse(text: &str) -> Result<Decl, String> {
    let d: Decl = serde_json::from_str(text).map_err(|e| format!("unreadable: {e}"))?;
    d.validate()?;
    Ok(d)
  }

  pub fn validate(&self) -> Result<(), String> {
    if self.version != 1 {
      return Err("version must be 1".into());
    }
    match (&self.expires, &self.keep) {
      (Some(e), None) => {
        parse_time(e).ok_or("expires is not an RFC 3339 time")?;
      }
      (None, Some(k)) => {
        if k.reason.trim().is_empty() {
          return Err("keep needs a reason".into());
        }
        if let Some(r) = &k.review_after {
          parse_time(r).ok_or("review_after is not a date or RFC 3339 time")?;
        }
      }
      _ => return Err("exactly one of expires or keep".into()),
    }
    self.disposition()?;
    for r in self.children.iter().flatten() {
      let p = &r.pattern;
      if p.matches('*').count() != 1 || p == "*" || p.contains(['?', '[', ']', '{', '}', '/']) {
        return Err(format!(
          "pattern {p:?} needs exactly one * plus literal text"
        ));
      }
      if r.max_age_days.is_none() && r.max_count.is_none() && r.max_bytes.is_none() {
        return Err(format!(
          "rule {p:?} needs max_age_days, max_count, or max_bytes"
        ));
      }
      if r.max_count.is_some_and(|c| c < r.keep_newest.unwrap_or(0)) {
        return Err(format!("rule {p:?}: max_count is below keep_newest"));
      }
    }
    Ok(())
  }

  pub fn disposition(&self) -> Result<Disposition, String> {
    match self.disposition.as_deref() {
      None | Some("quarantine") => Ok(Disposition::Quarantine),
      Some("delete") => Ok(Disposition::Delete),
      Some(other) => Err(format!("disposition {other:?} is not quarantine or delete")),
    }
  }

  fn expired(&self, now: i64) -> bool {
    self
      .expires
      .as_deref()
      .and_then(parse_time)
      .is_some_and(|t| t <= now)
  }

  fn rules(&self) -> &[Rule] {
    self.children.as_deref().unwrap_or(&[])
  }
}

/// Unix seconds for `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)`.
pub fn parse_time(s: &str) -> Option<i64> {
  let b = s.as_bytes();
  let num = |r: std::ops::Range<usize>| -> Option<i64> {
    let t = s.get(r)?;
    if t.bytes().all(|c| c.is_ascii_digit()) {
      t.parse().ok()
    } else {
      None
    }
  };
  if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
    return None;
  }
  let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
  if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
    return None;
  }
  let days = days_from_civil(y, mo, d);
  if b.len() == 10 {
    return Some(days * 86400);
  }
  if b.len() < 20 || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
    return None;
  }
  let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
  if h > 23 || mi > 59 || sec > 60 {
    return None;
  }
  let mut i = 19;
  if b[i] == b'.' {
    i += 1;
    let start = i;
    while i < b.len() && b[i].is_ascii_digit() {
      i += 1;
    }
    if i == start {
      return None;
    }
  }
  let offset = match s.get(i..)? {
    "Z" => 0,
    tz if tz.len() == 6 && (tz.starts_with('+') || tz.starts_with('-')) && &tz[3..4] == ":" => {
      let sign = if tz.starts_with('-') { -1 } else { 1 };
      sign * (num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60)
    }
    _ => return None,
  };
  Some(days * 86400 + h * 3600 + mi * 60 + sec - offset)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
  let y = if m <= 2 { y - 1 } else { y };
  let era = y.div_euclid(400);
  let yoe = y - era * 400;
  let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
  let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
  era * 146097 + doe - 719468
}

fn matches(pattern: &str, name: &str) -> bool {
  let (head, tail) = pattern
    .split_once('*')
    .expect("validated pattern has one *");
  name.len() > head.len() + tail.len() && name.starts_with(head) && name.ends_with(tail)
}

/// A tree as the evaluator sees it: the fixture model or a directory scan.
pub trait View {
  /// Direct children of `dir`, excluding its `.reap` file.
  fn children(&self, dir: &Path) -> Vec<PathBuf>;
  /// Newest modification time (unix seconds) anywhere inside `path`, including `path`.
  fn newest_mtime(&self, path: &Path) -> f64;
  /// Total file bytes inside `path`.
  fn bytes(&self, path: &Path) -> u64;
  /// Honoured declarations: path of the declaring directory -> parsed file or the reason it is
  /// unreadable. Declarations the honour rules reject (C1–C3) are absent.
  fn declarations(&self) -> BTreeMap<PathBuf, Result<Decl, String>>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct Removal {
  pub path: PathBuf,
  /// The declaring directory whose lifetime or rule removes this path.
  pub by: PathBuf,
  pub disposition: Disposition,
  pub reason: String,
}

#[derive(Debug, Default)]
pub struct Evaluation {
  pub removals: Vec<Removal>,
  /// Declaring directory -> why its declaration is invalid; its whole subtree is protected.
  pub invalid: BTreeMap<PathBuf, String>,
}

/// Apply SPEC-REAP-FILE C4–C12 to a view at time `now` (unix seconds).
pub fn evaluate(view: &dyn View, now: i64) -> Evaluation {
  let decls = view.declarations();
  let mut invalid: BTreeMap<PathBuf, String> = BTreeMap::new();
  let mut valid: BTreeMap<PathBuf, Decl> = BTreeMap::new();
  for (p, d) in &decls {
    match d {
      Ok(d) => {
        valid.insert(p.clone(), d.clone());
      }
      Err(e) => {
        invalid.insert(p.clone(), e.clone());
      }
    }
  }
  let sealed: Vec<PathBuf> = valid
    .iter()
    .filter(|(_, d)| d.seal == Some(true))
    .map(|(p, _)| p.clone())
    .collect();
  let under_seal = |p: &Path| sealed.iter().any(|s| s != p && p.starts_with(s));
  valid.retain(|p, _| !under_seal(p));
  invalid.retain(|p, _| !under_seal(p));
  let live: BTreeSet<PathBuf> = valid.keys().chain(invalid.keys()).cloned().collect();

  // A child matching two rules makes its parent's declaration invalid (C7).
  let ambiguous: Vec<PathBuf> = valid
    .iter()
    .filter(|(p, d)| {
      view.children(p).iter().any(|c| {
        let name = c
          .file_name()
          .map(|n| n.to_string_lossy().into_owned())
          .unwrap_or_default();
        d.rules()
          .iter()
          .filter(|r| matches(&r.pattern, &name))
          .count()
          > 1
      })
    })
    .map(|(p, _)| p.clone())
    .collect();
  for p in ambiguous {
    valid.remove(&p);
    invalid.insert(p, "a child matches more than one rule".into());
  }

  let protected: BTreeSet<PathBuf> = invalid
    .keys()
    .cloned()
    .chain(
      valid
        .iter()
        .filter(|(_, d)| !d.expired(now))
        .map(|(p, _)| p.clone()),
    )
    .collect();
  let mut out: Vec<Removal> = Vec::new();
  let mut prune = |root: &Path, by: &Path, disposition: Disposition, reason: &str| {
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
      if protected.iter().any(|q| q != &p && q.starts_with(&p)) {
        stack.extend(
          view
            .children(&p)
            .into_iter()
            .filter(|c| !protected.contains(c)),
        );
      } else {
        out.push(Removal {
          path: p,
          by: by.to_path_buf(),
          disposition,
          reason: reason.to_string(),
        });
      }
    }
  };

  for (p, d) in &valid {
    if invalid.keys().any(|i| p.starts_with(i)) {
      continue;
    }
    let disposition = d.disposition().expect("validated");
    if d.expired(now) {
      prune(
        p,
        p,
        disposition,
        &format!("expired {}", d.expires.as_deref().unwrap_or("")),
      );
      continue;
    }
    for r in d.rules() {
      let mut units: Vec<PathBuf> = view
        .children(p)
        .into_iter()
        .filter(|c| !live.contains(c))
        .filter(|c| {
          c.file_name()
            .is_some_and(|n| matches(&r.pattern, &n.to_string_lossy()))
        })
        .collect();
      units.sort_by(|a, b| {
        view
          .newest_mtime(b)
          .total_cmp(&view.newest_mtime(a))
          .then_with(|| a.cmp(b))
      });
      let keep: BTreeSet<PathBuf> = units
        .iter()
        .take(r.keep_newest.unwrap_or(0))
        .cloned()
        .collect();
      let mut evict: BTreeMap<PathBuf, String> = BTreeMap::new();
      if let Some(days) = r.max_age_days {
        for u in units.iter().filter(|u| !keep.contains(*u)) {
          let age = (now as f64 - view.newest_mtime(u)) / 86400.0;
          if age > days {
            evict.insert(u.clone(), format!("{} older than {days} days", r.pattern));
          }
        }
      }
      if let Some(cap) = r.max_count {
        let mut survivors: Vec<&PathBuf> =
          units.iter().filter(|u| !evict.contains_key(*u)).collect();
        for u in units.iter().rev() {
          if survivors.len() <= cap {
            break;
          }
          if !keep.contains(u) && !evict.contains_key(u) {
            evict.insert(u.clone(), format!("{} beyond {cap} units", r.pattern));
            survivors.retain(|s| *s != u);
          }
        }
      }
      if let Some(budget) = r.max_bytes {
        let mut total: u64 = units
          .iter()
          .filter(|u| !evict.contains_key(*u))
          .map(|u| view.bytes(u))
          .sum();
        for u in units.iter().rev() {
          if total <= budget {
            break;
          }
          if !keep.contains(u) && !evict.contains_key(u) {
            total -= view.bytes(u);
            evict.insert(u.clone(), format!("{} over {budget} bytes", r.pattern));
          }
        }
      }
      for (u, reason) in evict {
        if !protected.contains(&u) {
          prune(&u, p, disposition, &reason);
        }
      }
    }
  }
  // A removed ancestor subsumes removed descendants.
  let roots: Vec<PathBuf> = out.iter().map(|r| r.path.clone()).collect();
  out.retain(|r| !roots.iter().any(|a| a != &r.path && r.path.starts_with(a)));
  out.sort_by(|a, b| a.path.cmp(&b.path));
  out.dedup_by(|a, b| a.path == b.path);
  Evaluation {
    removals: out,
    invalid,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  struct FixtureView {
    nodes: BTreeMap<PathBuf, serde_json::Value>,
    now: i64,
  }

  impl FixtureView {
    fn under<'a>(
      &'a self,
      p: &'a Path,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a serde_json::Value)> {
      self.nodes.iter().filter(move |(q, _)| q.starts_with(p))
    }
  }

  impl View for FixtureView {
    fn children(&self, dir: &Path) -> Vec<PathBuf> {
      self
        .nodes
        .keys()
        .filter(|q| q.parent() == Some(dir))
        .cloned()
        .collect()
    }
    fn newest_mtime(&self, path: &Path) -> f64 {
      self
        .under(path)
        .map(|(_, n)| self.now as f64 - n["age_days"].as_f64().unwrap() * 86400.0)
        .fold(f64::MIN, f64::max)
    }
    fn bytes(&self, path: &Path) -> u64 {
      self
        .under(path)
        .filter(|(_, n)| n["type"] == "file")
        .map(|(_, n)| n["bytes"].as_u64().unwrap_or(0))
        .sum()
    }
    fn declarations(&self) -> BTreeMap<PathBuf, Result<Decl, String>> {
      self
        .nodes
        .iter()
        .filter_map(|(p, n)| {
          n.get("reap")
            .map(|r| (p.clone(), Decl::parse(&r.to_string())))
        })
        .collect()
    }
  }

  #[test]
  fn spec_fixtures() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/specs/reap-file.fixtures.json");
    let doc: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let now = parse_time(doc["now"].as_str().unwrap()).unwrap();
    let mut failures = vec![];
    for case in doc["cases"].as_array().unwrap() {
      let view = FixtureView {
        nodes: case["tree"]
          .as_object()
          .unwrap()
          .iter()
          .map(|(k, v)| (PathBuf::from(k), v.clone()))
          .collect(),
        now,
      };
      let ev = evaluate(&view, now);
      let got: Vec<String> = ev
        .removals
        .iter()
        .map(|r| r.path.display().to_string())
        .collect();
      let mut want: Vec<String> = case["expect"]["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
      want.sort();
      let got_invalid: Vec<String> = ev.invalid.keys().map(|p| p.display().to_string()).collect();
      let mut want_invalid: Vec<String> = case["expect"]["invalid"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
      want_invalid.sort();
      if got != want || got_invalid != want_invalid {
        failures.push(format!(
          "{}: removed {got:?} want {want:?}; invalid {got_invalid:?} want {want_invalid:?}",
          case["id"]
        ));
      }
    }
    assert!(
      failures.is_empty(),
      "fixture failures:\n{}",
      failures.join("\n")
    );
  }

  #[test]
  fn parses_times() {
    assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_time("2026-10-01"), Some(1_790_812_800));
    assert_eq!(parse_time("2026-10-01T02:00:00+02:00"), Some(1_790_812_800));
    assert_eq!(parse_time("2026-10-01T00:00:00.123Z"), Some(1_790_812_800));
    for bad in [
      "2026-13-01",
      "2026-10-01T25:00:00Z",
      "2026-10-01T00:00:00",
      "soon",
      "",
    ] {
      assert_eq!(parse_time(bad), None, "{bad}");
    }
  }
}
