#!/usr/bin/env python3
"""Reference evaluator for `.reap` declarations (SPEC-REAP-FILE). Works on a tree model, not disk.

Usage: evaluate.py [FIXTURES]   (default docs/specs/reap-file.fixtures.json; exit 0 = all cases pass)

A tree model maps relative paths to {"type": "dir"|"file", "age_days": n, "bytes": n, "reap": {...}}.
`age_days` is how long ago the entry was last modified; a unit's age is the newest modification
inside it. Honour rules (governed root, git-tracked, grace) are proven separately in
prototypes/lifetime-authority; every declaration in a model is treated as honoured unless a sealed
ancestor overrides it.
"""
import datetime
import json
import os
import sys

TOP = {"version", "expires", "keep", "seal", "scratch", "children", "disposition", "owner", "purpose"}
RULE = {"pattern", "keep_newest", "max_age_days", "max_count", "max_bytes"}


def parse_time(s):
  return datetime.datetime.fromisoformat(s.replace("Z", "+00:00"))


def validate(decl):
  """Error string, or None when the declaration is valid."""
  if not isinstance(decl, dict) or decl.get("version") != 1:
    return "version must be 1"
  if set(decl) - TOP:
    return "unknown field %s" % sorted(set(decl) - TOP)[0]
  if ("expires" in decl) == ("keep" in decl):
    return "exactly one of expires or keep"
  if "expires" in decl:
    try:
      parse_time(decl["expires"])
    except (TypeError, ValueError):
      return "expires is not an RFC 3339 time"
  if "keep" in decl:
    k = decl["keep"]
    if not isinstance(k, dict) or not k.get("reason") or set(k) - {"reason", "review_after"}:
      return "keep needs a reason (and optional review_after)"
  if decl.get("disposition", "quarantine") not in ("quarantine", "delete"):
    return "disposition is quarantine or delete"
  for r in decl.get("children", []):
    if not isinstance(r, dict) or set(r) - RULE or "pattern" not in r:
      return "child rule needs a pattern and known fields only"
    p = r["pattern"]
    if p.count("*") != 1 or any(c in p for c in "?[]{}/") or p == "*":
      return "pattern needs exactly one * plus literal text"
    if not any(k in r for k in ("max_age_days", "max_count", "max_bytes")):
      return "child rule needs max_age_days, max_count, or max_bytes"
    if "max_count" in r and r["max_count"] < r.get("keep_newest", 0):
      return "max_count is below keep_newest"
  return None


def matches(pattern, name):
  head, tail = pattern.split("*")
  return len(name) > len(head) + len(tail) and name.startswith(head) and name.endswith(tail)


def evaluate(tree, now):
  """Return (removed paths, invalid declaration dirs)."""
  now = parse_time(now)
  dirs = {p for p, e in tree.items() if e["type"] == "dir"}
  kids = {}
  for p in tree:
    kids.setdefault(os.path.dirname(p), []).append(p)
  under = lambda p, a: p == a or p.startswith(a + "/")
  decls = {p: e["reap"] for p, e in tree.items() if e["type"] == "dir" and "reap" in e}
  sealed = [p for p, d in decls.items() if not validate(d) and d.get("seal")]
  live = {p: d for p, d in decls.items() if not any(p != s and under(p, s) for s in sealed)}
  invalid = {p for p, d in live.items() if validate(d)}
  for p, d in live.items():
    rules = [] if p in invalid else d.get("children", [])
    for c in kids.get(p, []):
      if sum(matches(r["pattern"], os.path.basename(c)) for r in rules) > 1:
        invalid.add(p)
  valid = {p: d for p, d in live.items() if p not in invalid}

  def expired(p):
    d = valid[p]
    return "expires" in d and parse_time(d["expires"]) <= now

  # protected: a declared subtree that is invalid (protects everything) or not expired
  protected = invalid | {p for p in valid if not expired(p)}

  def unit_age(p):
    return min(tree[q]["age_days"] for q in tree if under(q, p))

  def unit_bytes(p):
    return sum(tree[q].get("bytes", 0) for q in tree if under(q, p) and tree[q]["type"] == "file")

  removed = set()

  def prune(p):
    """Remove p, or recurse around protected subtrees inside it."""
    if not any(q != p and under(q, p) for q in protected):
      removed.add(p)
      return
    for c in kids.get(p, []):
      if c not in protected:
        prune(c)

  for p, d in valid.items():
    if any(under(p, i) for i in invalid):
      continue
    if expired(p):
      prune(p)
      continue
    for r in d.get("children", []):
      units = [c for c in kids.get(p, []) if os.path.basename(c) != ".reap"
               and matches(r["pattern"], os.path.basename(c)) and c not in live]
      units.sort(key=unit_age)
      keep = set(units[:r.get("keep_newest", 0)])
      evict = set()
      if "max_age_days" in r:
        evict |= {u for u in units if u not in keep and unit_age(u) > r["max_age_days"]}
      if "max_count" in r:
        survivors = [u for u in units if u not in evict]
        for u in reversed(survivors[:]):
          if len(survivors) <= r["max_count"]:
            break
          if u not in keep:
            evict.add(u)
            survivors.remove(u)
      if "max_bytes" in r:
        survivors = [u for u in units if u not in evict]
        total = sum(unit_bytes(u) for u in survivors)
        for u in reversed(survivors):
          if total <= r["max_bytes"]:
            break
          if u not in keep:
            evict.add(u)
            total -= unit_bytes(u)
      for u in evict:
        if u not in protected:
          prune(u)

  # a removed ancestor subsumes removed descendants
  removed = {p for p in removed if not any(a != p and under(p, a) for a in removed)}
  return sorted(removed), sorted(invalid)


def main():
  path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "..", "..", "docs", "specs", "reap-file.fixtures.json")
  doc = json.load(open(path))
  fails = 0
  for case in doc["cases"]:
    removed, invalid = evaluate(case["tree"], doc["now"])
    ok = removed == sorted(case["expect"]["removed"]) and invalid == sorted(case["expect"].get("invalid", []))
    fails += not ok
    print("%-4s %-34s %s" % ("ok" if ok else "FAIL", case["id"], case["clauses"]))
    if not ok:
      print("     removed  got %s want %s" % (removed, sorted(case["expect"]["removed"])))
      print("     invalid  got %s want %s" % (invalid, sorted(case["expect"].get("invalid", []))))
  print("RESULT", "PASS" if fails == 0 else "FAIL", "(%d of %d cases)" % (len(doc["cases"]) - fails, len(doc["cases"])))
  return 1 if fails else 0


if __name__ == "__main__":
  sys.exit(main())
