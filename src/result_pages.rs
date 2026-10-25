//! Bounded JSON result pages. The terminal summary binds the complete original byte stream.

use std::io;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const INLINE_BYTES: usize = 4096;
pub const MAX_BYTES: usize = 512 * 1024;
pub const PAGE_BYTES: usize = 8192;
pub const MAX_PAGES: usize = MAX_BYTES / (PAGE_BYTES - 3) + 1;

/// Identity and bounds of one complete retained JSON result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
  pub bytes: usize,
  pub pages: usize,
  pub sha256: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Summary {
  paged_result: Manifest,
}

/// One complete retained page, not a new execution request or cleanup capability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
  pub index: usize,
  pub manifest: Manifest,
  pub text: String,
}

/// One ordered private frame; the terminal summary binds its contents.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fragment {
  pub index: usize,
  pub text: String,
}

/// Keep small results inline. Larger complete JSON results retain their original UTF-8 bytes.
pub fn prepare(detail: String) -> io::Result<(String, Vec<String>)> {
  if detail.len() <= INLINE_BYTES {
    if descriptor(&detail)?.is_some() {
      return Err(invalid("a page summary requires its complete result"));
    }
    return Ok((detail, vec![]));
  }
  if detail.len() > MAX_BYTES {
    return Err(invalid(
      "capacity-incomplete: complete result exceeds its bound",
    ));
  }
  serde_json::from_str::<serde::de::IgnoredAny>(&detail)
    .map_err(|_| invalid("paged result is not complete JSON"))?;
  let mut pages = Vec::new();
  let mut remaining = detail.as_str();
  while !remaining.is_empty() {
    let mut end = remaining.len().min(PAGE_BYTES);
    while !remaining.is_char_boundary(end) {
      end -= 1;
    }
    pages.push(remaining[..end].to_owned());
    remaining = &remaining[end..];
  }
  let manifest = Manifest {
    bytes: detail.len(),
    pages: pages.len(),
    sha256: format!("{:x}", Sha256::digest(detail.as_bytes())),
  };
  let summary = serde_json::to_string(&Summary {
    paged_result: manifest,
  })?;
  validate(&summary, &pages)?;
  Ok((summary, pages))
}

/// A reserved summary must be complete and canonical in shape, even without its payload.
pub fn descriptor(detail: &str) -> io::Result<Option<Manifest>> {
  if detail.len() > INLINE_BYTES {
    return Err(invalid("terminal summary exceeds its bound"));
  }
  let Ok(value) = serde_json::from_str::<serde_json::Value>(detail) else {
    return Ok(None);
  };
  if value.get("paged_result").is_none() {
    return Ok(None);
  }
  let summary: Summary =
    serde_json::from_value(value).map_err(|_| invalid("invalid paged result summary"))?;
  let manifest = summary.paged_result;
  if manifest.bytes <= INLINE_BYTES
    || manifest.bytes > MAX_BYTES
    || manifest.pages == 0
    || manifest.pages > MAX_PAGES
    || manifest.sha256.len() != 64
    || !manifest
      .sha256
      .bytes()
      .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
  {
    return Err(invalid("invalid paged result bounds or digest"));
  }
  Ok(Some(manifest))
}

/// Validate all bytes before publication. Missing, reordered, or substituted pages cannot complete.
pub fn validate(detail: &str, pages: &[String]) -> io::Result<()> {
  let manifest = descriptor(detail)?;
  if pages.is_empty() {
    return if manifest.is_none() {
      Ok(())
    } else {
      Err(invalid("paged result has no payload"))
    };
  }
  let manifest = manifest.ok_or_else(|| invalid("result pages have no terminal summary"))?;
  if manifest.pages != pages.len()
    || pages
      .iter()
      .any(|page| page.is_empty() || page.len() > PAGE_BYTES)
    || pages.iter().map(String::len).sum::<usize>() != manifest.bytes
  {
    return Err(invalid("incomplete result pages"));
  }
  let body = pages.concat();
  if format!("{:x}", Sha256::digest(body.as_bytes())) != manifest.sha256 {
    return Err(invalid("result pages differ from their terminal digest"));
  }
  serde_json::from_str::<serde::de::IgnoredAny>(&body)
    .map_err(|_| invalid("paged result is not complete JSON"))?;
  Ok(())
}

pub fn validate_page(detail: &str, page: &Page) -> io::Result<()> {
  if descriptor(detail)?.as_ref() != Some(&page.manifest)
    || page.index >= page.manifest.pages
    || page.text.is_empty()
    || page.text.len() > PAGE_BYTES
  {
    return Err(invalid("page differs from its retained result"));
  }
  Ok(())
}

fn invalid(reason: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn complete_utf8_json_round_trips_at_the_capacity_boundary() {
    let body = serde_json::to_string(&"界".repeat((MAX_BYTES - 2) / 3)).unwrap();
    let (summary, pages) = prepare(body.clone()).unwrap();
    assert!(summary.len() <= INLINE_BYTES);
    assert!(pages.len() > 1 && pages.len() <= MAX_PAGES);
    assert_eq!(pages.concat(), body);
    validate(&summary, &pages).unwrap();
    assert!(prepare(serde_json::to_string(&"x".repeat(MAX_BYTES)).unwrap()).is_err());
    assert_eq!(
      prepare("small result".into()).unwrap(),
      ("small result".into(), vec![])
    );
  }

  #[test]
  fn incomplete_substituted_or_unbound_pages_never_establish_completion() {
    let body = serde_json::to_string(&format!(
      "{}{}",
      "a".repeat(PAGE_BYTES),
      "b".repeat(PAGE_BYTES)
    ))
    .unwrap();
    let (summary, pages) = prepare(body).unwrap();
    assert!(validate(&summary, &pages[..pages.len() - 1]).is_err());
    let mut changed = pages.clone();
    changed.swap(0, 1);
    assert!(validate(&summary, &changed).is_err());
    changed[0] = "substituted result".into();
    assert!(validate(&summary, &changed).is_err());
    assert!(validate(&summary, &[]).is_err());
    assert!(validate("unrelated summary", &pages).is_err());
    assert!(prepare(summary).is_err());
    assert!(prepare("not JSON".repeat(INLINE_BYTES)).is_err());
  }
}
