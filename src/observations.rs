//! Bounded first-seen facts. Losing these facts restarts grace; it never grants disposal.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::identity::InstanceIdentity;

/// Fixed durable-index capacity; eviction restarts grace for forgotten keys.
pub(crate) const MAX_KEYS: usize = 8192;
/// One private reply fits the existing 64 KiB frame.
pub(crate) const BATCH_KEYS: usize = 512;
/// Reserve worker cache and observed-key allocations inside the Files preview budget.
pub(crate) const WORKER_BYTES: usize = 4 * 1024 * 1024;

/// Facts committed with one complete Files result. Timestamps come from the metadata writer.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Observations {
  /// Metadata-writer publication time, in Unix seconds.
  pub at: u64,
  pub first_seen: BTreeMap<String, u64>,
}

/// A bounded index over retained facts, not cached selection or disposal verdicts.
#[derive(Default)]
pub(crate) struct Cache {
  entries: BTreeMap<String, (u64, u64)>,
  order: BTreeSet<(u64, String)>,
}

impl Observations {
  pub(crate) fn validate(&self) -> io::Result<()> {
    if self.at > i64::MAX as u64
      || self.first_seen.len() > MAX_KEYS
      || self
        .first_seen
        .iter()
        .any(|(key, time)| !valid_key(key) || *time > i64::MAX as u64)
    {
      return Err(io::Error::other("invalid first-seen evidence"));
    }
    Ok(())
  }
}

impl Cache {
  pub(crate) fn get(&self, key: &str) -> Option<u64> {
    self.entries.get(key).map(|(first, _)| *first)
  }

  /// Return a bounded key-ordered page. The cursor excludes the previous page's final key.
  pub(crate) fn page(&self, after: Option<&str>) -> (BTreeMap<String, u64>, Option<String>) {
    use std::ops::Bound::{Excluded, Unbounded};

    let bounds = (after.map_or(Unbounded, Excluded), Unbounded);
    let mut entries = self.entries.range::<str, _>(bounds);
    let values: BTreeMap<_, _> = entries
      .by_ref()
      .take(BATCH_KEYS)
      .map(|(key, (first, _))| (key.clone(), *first))
      .collect();
    let next = entries
      .next()
      .and_then(|_| values.keys().next_back().cloned());
    (values, next)
  }

  /// Fold durable records in any order. Capacity retains the most recently observed keys.
  pub(crate) fn include(&mut self, observations: &Observations) {
    for (key, first) in &observations.first_seen {
      let (first, at) = match self.entries.get(key) {
        Some((old_first, old_at)) => {
          self.order.remove(&(*old_at, key.clone()));
          ((*first).min(*old_first), observations.at.max(*old_at))
        }
        None => (*first, observations.at),
      };
      self.entries.insert(key.clone(), (first, at));
      self.order.insert((at, key.clone()));
      if self.entries.len() > MAX_KEYS {
        let (_, key) = self.order.pop_first().expect("one excess observation");
        self.entries.remove(&key);
      }
    }
  }

  /// New keys start at publication, never at worker input, path age, or job admission.
  pub(crate) fn prepare(&self, keys: &[String], now: u64) -> io::Result<Observations> {
    if keys.len() > MAX_KEYS || keys.iter().any(|key| !valid_key(key)) {
      return Err(io::Error::other("invalid first-seen publication keys"));
    }
    let facts = Observations {
      at: now,
      first_seen: keys
        .iter()
        .map(|key| (key.clone(), self.get(key).unwrap_or(now)))
        .collect(),
    };
    facts.validate()?;
    Ok(facts)
  }
}

/// Both native objects and the authored policy digest bind grace independently of pathname.
pub(crate) fn key(identity: &InstanceIdentity, policy_digest: &str) -> io::Result<String> {
  if !valid_key(policy_digest) {
    return Err(io::Error::other("invalid first-seen policy digest"));
  }
  let bytes = serde_json::to_vec(&(identity, policy_digest)).map_err(io::Error::other)?;
  Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn valid_key(key: &str) -> bool {
  key.len() == 64
    && key
      .bytes()
      .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::identity::ObjectIdentity;

  #[test]
  fn keys_bind_both_native_objects_and_policy_but_not_location() {
    let identity = InstanceIdentity {
      directory: ObjectIdentity {
        volume_uuid: "volume".into(),
        file_id: "directory".into(),
      },
      declaration: ObjectIdentity {
        volume_uuid: "volume".into(),
        file_id: "policy".into(),
      },
    };
    let digest = format!("{:x}", Sha256::digest(b"policy"));
    let original = key(&identity, &digest).unwrap();
    assert_eq!(original, key(&identity.clone(), &digest).unwrap());
    for field in ["directory", "declaration"] {
      let mut copied = identity.clone();
      if field == "directory" {
        copied.directory.file_id = "copy".into();
      } else {
        copied.declaration.file_id = "replacement".into();
      }
      assert_ne!(original, key(&copied, &digest).unwrap());
    }
    assert_ne!(
      original,
      key(
        &identity,
        &format!("{:x}", Sha256::digest(b"changed policy"))
      )
      .unwrap()
    );
  }

  #[test]
  fn durable_facts_survive_reordering_and_cache_loss_restarts_grace() {
    let key = "a".repeat(64);
    let mut cache = Cache::default();
    let first = cache.prepare(&[key.clone()], 100).unwrap();
    assert_eq!(first.first_seen[&key], 100);
    cache.include(&first);
    let later = cache.prepare(&[key.clone()], 200).unwrap();
    assert_eq!(later.first_seen[&key], 100);
    let mut recovered = Cache::default();
    recovered.include(&later);
    recovered.include(&first);
    assert_eq!(recovered.get(&key), Some(100));
    assert_eq!(
      Cache::default()
        .prepare(&[key.clone()], 300)
        .unwrap()
        .first_seen[&key],
      300
    );
    assert_eq!(
      cache.prepare(&[key.clone()], 50).unwrap().first_seen[&key],
      100
    );
    assert!(cache.prepare(&["unsafe".into()], 300).is_err());
  }

  #[test]
  fn capacity_drops_credit_without_exceeding_the_fixed_bound() {
    let mut cache = Cache::default();
    for number in 0..MAX_KEYS + 10 {
      let facts = Observations {
        at: number as u64,
        first_seen: BTreeMap::from([(format!("{number:064x}"), number as u64)]),
      };
      cache.include(&facts);
      assert!(cache.entries.len() <= MAX_KEYS);
    }
    assert_eq!(cache.get(&format!("{:064x}", 0)), None);
    assert_eq!(
      cache
        .prepare(&[format!("{:064x}", 0)], 99999)
        .unwrap()
        .first_seen
        .values()
        .copied()
        .next(),
      Some(99999)
    );
  }

  #[test]
  fn bounded_pages_reconstruct_the_index_without_repeating_cursor_keys() {
    let mut cache = Cache::default();
    let facts = cache
      .prepare(
        &(0..MAX_KEYS)
          .map(|number| format!("{number:064x}"))
          .collect::<Vec<_>>(),
        100,
      )
      .unwrap();
    cache.include(&facts);
    let mut recovered = BTreeMap::new();
    let mut after = None;
    let mut pages = 0;
    loop {
      let (values, next) = cache.page(after.as_deref());
      assert_eq!(values.len(), BATCH_KEYS);
      assert!(values
        .keys()
        .all(|key| after.as_ref().is_none_or(|cursor| key > cursor)));
      if let Some(next) = &next {
        assert_eq!(values.keys().next_back(), Some(next));
      }
      recovered.extend(values);
      pages += 1;
      if next.is_none() {
        break;
      }
      after = next;
    }
    assert_eq!(pages, MAX_KEYS / BATCH_KEYS);
    assert_eq!(recovered, facts.first_seen);
    assert!(Cache::default().page(None).0.is_empty());
    assert!(cache.page(Some(&"f".repeat(64))).0.is_empty());
  }
}
