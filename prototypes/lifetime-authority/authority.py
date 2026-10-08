#!/usr/bin/env python3
"""Prototype of the honour rules for directory-held `.reap` declarations (MS-LIFETIME-AUTHORITY).

Which `.reap` files may reap act on? A declaration is honoured only when all hold:

1. It lies under a governed root. The root path may itself be a symlink (the mini's ~/projects points
   at /Volumes/kytos/projects); nothing below the root is followed, so a symlink inside the tree never
   leads the scan elsewhere and a copy outside the root is never seen.
2. It is not tracked by git; a committed declaration travels with every clone.
3. Its file identity (device, inode) has been seen for at least the grace period. A rename or move
   on the same volume keeps the identity; a copy or a fresh file starts a new grace period.
4. No ancestor declaration below the root is sealed; a sealed declaration governs its whole subtree
   and descendants' declarations are ignored (a preservation copy inside the root).

The first-seen state is a rebuildable cache, never authority: losing it only delays action.
"""
import json
import os
import subprocess
import time

NAME = ".reap"


def tracked_by_git(path):
  d = os.path.dirname(path)
  top = subprocess.run(["git", "-C", d, "rev-parse", "--show-toplevel"], capture_output=True, text=True,
                       env={**os.environ, "GIT_OPTIONAL_LOCKS": "0"})
  if top.returncode != 0:
    return False
  r = subprocess.run(["git", "-C", top.stdout.strip(), "ls-files", "--error-unmatch", "--", path],
                     capture_output=True, text=True, env={**os.environ, "GIT_OPTIONAL_LOCKS": "0"})
  return r.returncode == 0


def declarations(root):
  """Every `.reap` under root without following any symlink below the root."""
  out = []
  stack = [os.path.realpath(root)]
  while stack:
    d = stack.pop()
    try:
      entries = list(os.scandir(d))
    except OSError:
      continue
    for e in entries:
      if e.name == NAME and e.is_file(follow_symlinks=False):
        out.append(e.path)
      elif e.is_dir(follow_symlinks=False) and e.name != ".git":
        stack.append(e.path)
  return sorted(out)


def evaluate(root, state_path, grace_secs, now=None):
  """Verdict per declaration directory: ('honoured' | 'ignored', reason)."""
  now = time.time() if now is None else now
  state = {}
  if os.path.exists(state_path):
    with open(state_path) as f:
      state = json.load(f)
  seen = {}
  verdicts = {}
  decls = declarations(root)
  sealed = []
  for path in decls:
    try:
      with open(path) as f:
        body = json.load(f)
    except (OSError, ValueError):
      body = None
    if body and body.get("seal"):
      sealed.append(os.path.dirname(path))
  for path in decls:
    d = os.path.dirname(path)
    st = os.lstat(path)
    key = "%d:%d" % (st.st_dev, st.st_ino)
    first = state.get(key, now)
    seen[key] = first
    if any(d != s and d.startswith(s + os.sep) for s in sealed):
      verdicts[d] = ("ignored", "sealed ancestor")
    elif tracked_by_git(path):
      verdicts[d] = ("ignored", "tracked by git")
    elif now - first < grace_secs:
      verdicts[d] = ("ignored", "within grace (first seen %ds ago)" % int(now - first))
    else:
      verdicts[d] = ("honoured", "")
  with open(state_path, "w") as f:
    json.dump(seen, f)
  return verdicts
