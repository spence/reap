#!/usr/bin/env python3
"""Positive cases and negative controls for the `.reap` honour rules. Throwaway trees only.

Usage: scenarios.py   (exit 0 when every verdict matches; prints a table either way)
The root is a symlink to a real directory, as the mini's ~/work is. A naive scanner (follows
symlinks, no git or grace check, no seal) runs over the same tree and must disagree with the
controls; otherwise the controls prove nothing.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

import authority

GRACE = 3600


def write(path, body):
  os.makedirs(os.path.dirname(path), exist_ok=True)
  with open(path, "w") as f:
    json.dump(body, f)


def git(cwd, *args):
  subprocess.run(["git", "-C", cwd, *args], check=True, capture_output=True,
                 env={**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t",
                      "GIT_COMMITTER_EMAIL": "t@t", "GIT_CONFIG_GLOBAL": "/dev/null"})


def naive(root):
  out = {}
  for d, dirs, files in os.walk(root, followlinks=True):
    if ".reap" in files:
      out[os.path.realpath(d)] = ("honoured", "")
  return out


def main():
  base = os.path.realpath(tempfile.mkdtemp(prefix="reap-authority-"))
  try:
    real = os.path.join(base, "kytos-work")
    root = os.path.join(base, "work")
    os.makedirs(real)
    os.symlink(real, root)
    elsewhere = os.path.join(base, "elsewhere")
    backup = os.path.join(base, "backup")
    exp = {"expires": "2026-10-07T00:00:00Z"}

    write(os.path.join(root, "p", "run1", ".reap"), exp)
    write(os.path.join(root, "p", "run2", ".reap"), exp)
    write(os.path.join(root, "p", "edited", ".reap"), exp)
    write(os.path.join(root, "p", "preserve", ".reap"), {"keep": "owner preservation copy", "seal": True})
    write(os.path.join(root, "p", "preserve", "run", ".reap"), exp)
    repo = os.path.join(root, "p", "repo")
    write(os.path.join(repo, ".reap"), exp)
    git(repo, "init", "-q")
    git(repo, "add", "-f", ".reap")
    git(repo, "commit", "-q", "-m", "tracked declaration")
    write(os.path.join(repo, "out", ".gitignore"), {})
    with open(os.path.join(repo, ".gitignore"), "w") as f:
      f.write("out/\n")
    write(os.path.join(repo, "out", ".reap"), exp)
    write(os.path.join(elsewhere, ".reap"), exp)
    os.symlink(elsewhere, os.path.join(root, "p", "link"))

    state = os.path.join(base, "state.json")
    t0 = 1_800_000_000
    authority.evaluate(root, state, GRACE, now=t0)

    # changes between the two passes
    os.rename(os.path.join(real, "p", "run1"), os.path.join(real, "p", "run1-renamed"))
    os.makedirs(os.path.join(real, "q"))
    os.rename(os.path.join(real, "p", "run2"), os.path.join(real, "q", "run2"))
    shutil.copytree(os.path.join(real, "p", "run1-renamed"), os.path.join(backup, "run1-backup"), symlinks=True)
    shutil.copytree(os.path.join(real, "p", "run1-renamed"), os.path.join(real, "p", "run1-copy"), symlinks=True)
    write(os.path.join(real, "p", "stray", ".reap"), exp)
    tmp = os.path.join(real, "p", "edited", ".reap.tmp")
    write(tmp, {"expires": "2026-10-01T00:00:00Z"})
    os.rename(tmp, os.path.join(real, "p", "edited", ".reap"))

    got = authority.evaluate(root, state, GRACE, now=t0 + 2 * GRACE)
    p = lambda *a: os.path.join(real, *a)
    expected = {
      p("p", "run1-renamed"): ("honoured", "rename keeps identity"),
      p("q", "run2"): ("honoured", "move within the root keeps identity"),
      p("p", "run1-copy"): ("ignored", "copy inside the root starts a new grace period"),
      p("p", "stray"): ("ignored", "new stray waits out the grace period"),
      p("p", "edited"): ("ignored", "atomic-rename edit restarts grace (delays only)"),
      p("p", "preserve"): ("honoured", "sealed declaration governs its subtree"),
      p("p", "preserve", "run"): ("ignored", "descendant of a sealed declaration"),
      p("p", "repo"): ("ignored", "git-tracked declaration"),
      p("p", "repo", "out"): ("honoured", "untracked declaration inside a repo"),
    }
    absent = {os.path.join(backup, "run1-backup"): "copy outside the root is never seen",
              os.path.realpath(elsewhere): "symlink inside the root is never followed",
              os.path.join(real, "p", "link"): "symlink inside the root is never followed"}

    fails = 0
    print("%-42s %-9s %-9s %s" % ("case", "expected", "got", "why"))
    for d, (want, why) in expected.items():
      verdict = got.get(d, ("absent", ""))[0]
      ok = verdict == want
      fails += not ok
      print("%-42s %-9s %-9s %s%s" % (os.path.relpath(d, real), want, verdict, why, "" if ok else "  <-- MISMATCH"))
    for d, why in absent.items():
      ok = d not in got
      fails += not ok
      print("%-42s %-9s %-9s %s%s" % (os.path.relpath(d, base), "absent", "absent" if ok else got[d][0], why,
                                       "" if ok else "  <-- MISMATCH"))

    n = naive(root)
    caught = sum(1 for d, (want, _) in expected.items() if want == "ignored" and n.get(d, ("absent",))[0] == "honoured")
    caught += sum(1 for d in absent if d in n)
    controls = sum(1 for want, _ in expected.values() if want == "ignored") + len(absent)
    print("\nnaive scanner (no rules) wrongly honours %d of %d negative controls" % (caught, controls))
    print("RESULT", "PASS" if fails == 0 and caught > 0 else "FAIL", "(%d mismatches)" % fails)
    return 0 if fails == 0 and caught > 0 else 1
  finally:
    shutil.rmtree(base)


if __name__ == "__main__":
  sys.exit(main())
