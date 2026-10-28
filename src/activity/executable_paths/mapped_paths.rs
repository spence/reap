//! Resolve missing executable names through complete, process-scoped native vnode evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

/// Require every selected PID, complete NUL framing, and known vnode type, identity, and links.
pub(in crate::activity) fn decode(
  bytes: &[u8],
  pids: &BTreeSet<i32>,
) -> io::Result<BTreeSet<(u64, u64)>> {
  let body = bytes.strip_suffix(b"\0\n").ok_or_else(incomplete)?;
  let (mut headers, mut covered, mut identities) =
    (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
  let mut pid = None;
  let mut record = BTreeMap::<u8, Vec<u8>>::new();
  for field in body.split(|byte| *byte == 0) {
    let field = field.strip_prefix(b"\n").unwrap_or(field);
    let (&kind, value) = field.split_first().ok_or_else(incomplete)?;
    if matches!(kind, b'p' | b'f') && !record.is_empty() {
      identities.insert(identity(&record)?);
      covered.insert(pid.ok_or_else(incomplete)?);
      record.clear();
    }
    if kind == b'p' {
      let value = text(value)?.parse::<i32>().map_err(io::Error::other)?;
      if !pids.contains(&value) || !headers.insert(value) {
        return Err(incomplete());
      }
      pid = Some(value);
    } else if pid.is_none()
      || !b"ftDik".contains(&kind)
      || (kind != b'f' && !record.contains_key(&b'f'))
      || record.insert(kind, value.to_vec()).is_some()
    {
      return Err(incomplete());
    }
  }
  if !record.is_empty() {
    identities.insert(identity(&record)?);
    covered.insert(pid.ok_or_else(incomplete)?);
  }
  if &headers != pids || &covered != pids {
    return Err(incomplete());
  }
  Ok(identities)
}

fn identity(record: &BTreeMap<u8, Vec<u8>>) -> io::Result<(u64, u64)> {
  if record.len() != 5
    || !b"ftDik".iter().all(|key| record.contains_key(key))
    || record[&b'f'] != b"txt"
    || record[&b't'] != b"REG"
  {
    return Err(incomplete());
  }
  let device = u64::from_str_radix(
    text(&record[&b'D'])?
      .strip_prefix("0x")
      .ok_or_else(incomplete)?,
    16,
  )
  .map_err(io::Error::other)?;
  let inode = text(&record[&b'i'])?
    .parse::<u64>()
    .map_err(io::Error::other)?;
  text(&record[&b'k'])?
    .parse::<u64>()
    .map_err(io::Error::other)?;
  if device == 0 || inode == 0 {
    return Err(incomplete());
  }
  Ok((device, inode))
}

fn text(bytes: &[u8]) -> io::Result<&str> {
  std::str::from_utf8(bytes).map_err(io::Error::other)
}

fn incomplete() -> io::Error {
  io::Error::other("incomplete native text-vnode records")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn missing_names_need_complete_vnode_identity_and_process_coverage() {
    let pids = BTreeSet::from([7, 8]);
    let bytes = b"p7\0\nftxt\0tREG\0D0x1\0i12\0k0\0\np8\0\nftxt\0tREG\0D0x1\0i13\0k1\0\n";
    assert_eq!(
      decode(bytes, &pids).unwrap(),
      BTreeSet::from([(1, 12), (1, 13)])
    );
    for bad in [
      bytes[..bytes.len() - 1].to_vec(),
      b"p7\0\nftxt\0tREG\0D0x1\0i12\0k0\0\n".to_vec(),
      replace(bytes, b"k0", b"k?"),
      replace(bytes, b"i12", b"i00"),
      replace(bytes, b"D0x1", b"D0x0"),
      replace(bytes, b"tREG", b"t???"),
      replace(bytes, b"p8", b"p7"),
      replace(bytes, b"p8", b"p9"),
    ] {
      assert!(decode(&bad, &pids).is_err());
    }
  }

  fn replace(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut bytes = bytes.to_vec();
    let start = bytes
      .windows(from.len())
      .position(|part| part == from)
      .unwrap();
    bytes[start..start + from.len()].copy_from_slice(to);
    bytes
  }
}
