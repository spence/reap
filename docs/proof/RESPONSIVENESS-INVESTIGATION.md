# Reap responsiveness investigation

Date: 2026-10-23. Work: `MS-LIFETIME-ENFORCE.T11`.

Status: investigation and design recommendation. No production behavior, installed binary,
scheduler, cleanup authority, or foreign process changed. Implementation and fleet rollout remain
separate work. The numbers below do not establish a performance improvement.

## Finding and recommendation

The main measured cost is broad filesystem discovery. Reap also puts filesystem operations,
subprocess completion, and shared-lock waits directly in the caller's execution path.
These are separate problems. Progress output addresses neither repeated work nor caller completion.

Use a small host-local job coordinator for prompt requests and status replies. Run target I/O in
bounded worker processes. Add safe workspace selectors, shared Cargo/store discovery, and explicit
coverage results. Preserve automatic discovery and every cleanup guard. Do not start with a
persistent artifact catalog, a watcher, or a CPU allowance.

This recommendation changes the older proposal's preference against another service.
Cross-command duplicate suppression and a response path independent of target I/O justify that
small component. It is not a recommendation for another timer or a remote service.

Two safety defects also require correction. An unreadable subtree can hide a KEEP.
A failed Git probe can make a tracked declaration appear untracked. Neither failure may become
cleanup permission when timeouts or faster discovery are introduced.

## Native evidence

The source baseline is recorded in [the evidence receipt](RESPONSIVENESS-2026-10-23.json).
It includes source digests, binary digests, commands, timings, phase counts, and fixture output.
Both installed fleet binaries had the same digest during this investigation.

### Installed Mini command

At 23:24:12 UTC, the installed `reap files /Users/spencer/projects` started a dry-run.
Its first stdout arrived after **106.966 seconds**. It exited after **106.974 seconds**.
It produced no stderr and proposed no removal. Recorded child CPU time was **65.667 seconds**.
The run used an isolated copy of lifecycle state, not the live first-seen cache.

This confirms that the installed command can remain silent for almost two minutes.
It does not require a stuck lock, a stuck Git process, or diagnostic instrumentation.

### Instrumented source copy on the Mini

The diagnostic build adds counters and inclusive spans. It does not change cleanup decisions.
It refuses apply commands and requires an explicit diagnostic environment variable.
A retained fixture produced identical stdout with the installed and diagnostic binaries.

| Operation | Process elapsed | Discovery | Other measured work | Discovery volume |
| --- | ---: | ---: | --- | --- |
| `files` | 112.315 s | 112.114 s | Git probes 3.055 s within discovery; evaluation 0.192 s | 2,160,928 directories; 8,995,362 entries |
| `sweep` | 95.636 s | 80.859 s | 74 target plans: 14.748 s | 637,711 directories; 10,591,642 entries |
| `stores` | 82.688 s | 79.575 s | Eight store plans: 2.853 s | 637,712 directories; 10,591,657 entries |

Span times overlap. Do not add Git time to `files` discovery time.
Manager receipts include coarse polling delay; the table uses the process's final profile time.

The largest `files` project was `micro-control-plane`: **1,815,607 directory visits**.
Large evidence and source-copy trees dominate that project's directory counts.
The problem is not limited to Cargo `target/` or `node_modules` directories.

Cargo and store discovery repeated nearly the same walk. Their largest costs included
`src/worktrees/micro-db`, `src/worktrees/held-orch`, and `src/worktrees/micro`.
Store sizing exists, but it was not the dominant cost in this run.
Skipping report-only sizing alone would not resolve this recurrence.

All three runs finished normally and reported zero eligible bytes for their armed cleanup paths.
There were no apply commands. Every recorded diagnostic process group had no remaining members.

### Earlier recurrence and limits

The latest maintenance receipt ran from 22:31:38 to 22:56:12 UTC:

| Stage | Elapsed |
| --- | ---: |
| Cargo | 669.887 s |
| Stores | 101.261 s |
| Files | 698.176 s |

Those stages reported zero affected bytes. A configuration change then stopped later stages.
That safe stop explains the failed outcome, not the preceding delay.

The [October 17 proposal](../plans/bounded-maintenance.md) records earlier slow discovery.
Its declaration stage took only 0.092 seconds. The historical input and cache state were not
preserved well enough to explain that difference. It is not evidence that the current scan is cheap.

The Mini had concurrent builds, Git work, substantial swap use, and disk activity.
Those conditions can change elapsed time. They do not explain away millions of repeated visits.
The historical 698-second phase cannot be divided retrospectively into exact CPU and I/O costs.
The macOS cause of the older kernel-blocked `lsof` incident also remains unknown.

These are diagnostic runs, not paired performance benchmarks. The Cargo and store walks saw
15 different entries and one different directory. Concurrent writers remained active.
No candidate improvement, unchanged workload, or discovery parity is claimed.

## Reproduced blocking and safety boundaries

[The fixture harness](../../tools/reproduce_responsiveness.py) uses isolated state and owned
process groups. Its healthy command is the positive control. Its blocked cases have deadlines
in the harness, not in Reap. No fixture invokes `--apply`.

| Control | Observation on both Macs |
| --- | --- |
| Healthy, retained workspace | Normal completion; retained data survives |
| Git helper that waits | `files` remains silent until the harness stops its owned group |
| Two matching requests with that helper | Both run simultaneously; no cross-command coalescing |
| FIFO at the first-seen cache path | `files` remains silent until the harness stops it |
| FIFO at the completed-receipt path | `status` prints a header, then remains blocked |
| Owned lifecycle lock held | Read-only `lease list` waits about 30 seconds, then reports busy |
| Unreadable child containing KEEP | Dry-run proposes quarantine of its expired parent, with exit 0 |
| Git exit 128 for a tracked declaration | The ignored tracked declaration becomes honored; dry-run proposes quarantine |

FIFO cases model a stalled read. They do not simulate an uninterruptible kernel fault.
The unreadable-directory case confirms a real permission error before the dry-run.
The failed-Git case first proves that the real Git command identifies the declaration as tracked.
Its synthetic failure then emits a fatal diagnostic and exits 128.

The safety cases preserve their data. They establish unsafe proposals, not observed production
data loss or a successful destructive execution. Their exact output remains in the evidence.
The affected specification conformance is recorded in [SPEC-REAP-FILE](../specs/reap-file.md).

## Command and blocking audit

The audit covers dispatch in `src/main.rs` and each command's entry module.
Except help/version, every family can depend on ordinary filesystem calls.
No family has a whole-command deadline.

| Command family | Work on the caller's path | Lock or state effect |
| --- | --- | --- |
| `--help`, `--version` | Argument parsing; no project discovery | No lifecycle work |
| Bare `reap`, `sweep`, `list` | Global Cargo discovery; planning for sweep | Apply holds per-profile Cargo locks; no shared scan admission |
| `plan`, `clean` | Selected target planning, manifest reads, optional removal | `clean` implies apply; completion must not become acceptance silently |
| `check`, `config` | Local manifest/config reads, path metadata | Init writes config; some config reads can fall back to defaults |
| `stores` | Global manifest discovery or selected project; recursive child sizing | Apply holds lifecycle state across validation and removal/move |
| `stores --init`, `--bind` | Directory identity, manifest, marker and binding I/O | Writes authority; binding uses lifecycle state lock |
| `files` | Whole governed-root scan, Git probes, evaluation, repeated unit measurements | Dry-run writes first-seen cache; apply locks during candidate handling |
| `lease` including list | State load, path/marker operations, attribution helpers | All variants acquire the lifecycle state lock |
| `parents` including list | State and identity reads; direct-child enumeration | All variants acquire the lifecycle state lock |
| `create` | Git worktree/clone or recursive copy and verification | Durable creation intent; long producer work already runs outside state lock |
| `retire` including dry-run | Lease selection, deep assessment, Git/activity probes, optional move | State lock spans assessment and execution |
| `doctor` including dry-run | Live paths, markers, mount ancestry, quarantine metadata | State lock spans diagnosis; apply can repair records |
| `quarantine` including list | Index and slot reads; restore can copy across volumes | State lock spans list, holds, or restore |
| `purge` including dry-run | Index, selection, slot validation, optional recursive deletion | State lock spans selection and execution |
| `inventory --quick` | Recursive tree measurement and Git inspection still occur | Quick hides byte reporting; it does not omit the tree walk |
| `status` | Live capacity, leases, markers, quarantine slots, completed receipt | No lifecycle lock or recursive sizing; still synchronous I/O |
| `coverage` | Shallow listings, canonical paths and registration identity | Read-only, but not independent of target I/O |
| `maintain` | Live capacity, sequential stage processes, completed receipt | Its lock excludes other maintenance, not standalone scans |

Specific mechanisms:

- `files::run` prints its first header after `scan` and `evaluate`.
- `files::scan` skips `.git`, but visits other directories regardless of their value to this request.
- A positional `files` path must equal a governed root. It cannot select one workspace within it.
- `Command::output()` waits for child completion and collects output. Git calls lack deadlines.
- Maintenance applies its log limit after collection, not during capture. Capture memory is not bounded by that limit.
- `state.lock` acquisition has a 30-second budget only after the file opens.
- Some state-lock owners perform target I/O or deep work while they hold it.
- Standalone scans can overlap maintenance. First-seen-cache writers also share a fixed temporary filename.
- Discovery and tree measurement skip some read errors. Missing protection is not proof of absence.

The existing five-second activity-probe budget handles its own child and inherited pipes.
It is not a deadline for Git, discovery, lock-file opening, mutation, or the whole command.
The existing [wait specification](../specs/lifecycle-waits.md) states those limits correctly.

## What an immediate response can mean

An exact, fresh scan of an arbitrary tree cannot always finish immediately.
The tree can be large, change during discovery, disappear, or stop answering I/O.
A timeout in the same thread cannot interrupt every blocking filesystem call.
An `async` function does not by itself remove those calls from the response path.

The useful guarantee is **a prompt, actionable reply independent of target work**:

- A completed result identifies its scope, observation time, and coverage.
- An accepted job returns its identifier and a status command, then the client exits.
- A duplicate returns the existing compatible job, not another scan.
- Busy, unavailable, or incomplete results return promptly and do not imply success.
- Freshness comes from an explicit refresh job, not hidden synchronous work inside `status`.
- `--wait` is an explicit choice to wait for job completion.

Suggested acceptance targets are p99 reply time below 250 ms and a one-second client response
budget after process startup. These are proposed targets, not shipped guarantees.
They assume a functioning OS, CPU scheduling, and local control channel.
No user-space program can promise execution while the OS itself cannot run it.

A printed header is insufficient. Tool callers need a returned receipt or job identifier.
Worker processes must not retain the client's stdout/stderr pipes after its reply.
Acceptance is not completion, successful renewal, successful cleanup, or confirmed free space.

## Recommended execution design

### Separate control from work

Use one host-local coordinator, with an on-demand platform lifecycle.
Its request loop serves in-memory job state and bounded IPC messages.
It performs no target traversal, Git operation, mount probe, or synchronous target-state read.
Cold or unavailable state produces an explicit unavailable/pending result, not a hidden live scan.

Isolate target work and durable I/O in owned worker processes. Start with one bulk worker per
managed volume and a bounded pending set. A stuck worker continues to occupy its slot.
Do not create replacement workers on every timeout or retry. Other healthy volumes retain their
own slots. No CPU/load admission rule is required.

Admission must establish a durable job record before it claims durable acceptance.
If that write stalls, the caller receives unavailable/unknown acceptance within its response budget.
An idempotency key makes a retry safe after an ambiguous response.
The coordinator must not claim persistence merely because it received a request.

Coalesce only equivalent requests: command, scope, mode, policy/config generation, and authority
must agree. A dry-run cannot become an apply request. Do not merge different explicit purge authority.
All CLI and scheduled callers must use the same admission path.

### Reduce work, not just foreground waits

1. Add a workspace selector inside a governed root. Inspect its required ancestors and descendants.
   Include sibling units when a governing count/byte/newest rule requires them.
   A selector must not bypass a sealed ancestor, overlapping lease, or inherited rule.
2. Share one discovery result between Cargo and stores within a maintenance job.
   Preserve standalone scope, exclusions, marker validation, and fresh executor guards.
3. Prune declaration discovery only where an honored policy proves that descendants cannot affect
   this command's result. A normal KEEP does not suffice: nested expiry remains meaningful.
   Directory names alone cannot justify omission of nested KEEP declarations.
4. Avoid deep sizing needed only for display. Retention, quiet-window and mount validation still
   require their real inputs. This is a smaller current win than duplicate discovery.
5. Keep discovery hints separate from cleanup authority. Do not use directory mtime as proof that
   descendants are unchanged. Unknown Git state and incomplete coverage protect affected candidates.

Begin without a persistent artifact index. The measured shared traversal and scoped path are
direct opportunities. If recurring discovery remains costly, measure an advisory incremental index
as a separate design choice. Registration cannot silently become mandatory for existing declarations.

### Completion, progress and cancellation

Expose queued, running, waiting, blocked, partial, completed, failed, interrupted, and cancellation
states. Include stage, location, elapsed time, work counts, observation age and incomplete coverage.
Progress records must not require another scan. Bound output capture before storage, not afterward.

A cancellation reply acknowledges the request, not a completed rollback.
Read-only discovery can stop between units. A mutation stops only at a safe journal boundary.
Do not kill a copy/move merely to meet a response target. A timed-out or kernel-blocked child is
not reported as gone until its identity and exit are verified.
Only owned workers and helpers may receive process-control signals.

Snapshots never authorize deletion. Before mutation, verify current declaration/lease authority,
retention, recoverability, identity, mounts, activity, and the config/binary generation.
Keep the existing move recovery and quarantine evidence intact.

Lease requests need special truthfulness: a queued renewal has not renewed the lease.
Conflicting lifecycle requests need ordered execution and fresh validation.
Unrelated control/status replies must not wait for a long lifecycle lock holder.
Shorter mutation critical sections may need durable per-path reservations before lock release.
That is a protocol change, not permission to remove the current lock around destructive work.

## Alternatives and delivery consequences

| Alternative | Assessment |
| --- | --- |
| Print sooner; increase tool timeouts | Helpful progress, but caller completion and repeated work remain unresolved |
| Faster synchronous discovery only | Necessary efficiency work; cannot promise a response when a required syscall stalls |
| Detach one worker per request | Small first patch, but retries multiply workers, and shared-file reads still block status |
| Host-local coordinator and bounded workers | Recommended: one admission boundary, bounded fan-out, independent status and explicit jobs |
| Mandatory registration | Changes automatic discovery; not selected |
| Persistent index plus FSEvents | Possible later optimization, not a fresh or authoritative inventory by itself |

Apple documents dropped/coalesced events and required rescans. A watcher does not eliminate
reconciliation or authorize cleanup. [Apple FSEvents guidance](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/UsingtheFSEventsFramework/UsingtheFSEventsFramework.html)
Rust documents the synchronous wait and output collection in `Command::output()`.
[Rust process documentation](https://doc.rust-lang.org/std/process/struct.Command.html#method.output)
Sending a signal is not evidence that a process has exited.
[Apple kill documentation](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/kill.2.html)

Nonblocking defaults change CLI completion semantics. Existing scripts, agent guidance, launchd
callers, exit-code interpretation and `clean` need an explicit migration.
The job record/protocol, recovery behavior, installer, signing and both native runtimes also need
verification. Older binaries must not bypass new reservations or write incompatible shared state.
The fleet operator guides belong to the Skills repository; the Reap installer must preserve them.

No launchd cadence, configured root, retention rule, or cleanup permission changes are required
for this investigation. The separate active-work expiry proposal remains deferred.

## Required proof before a production claim

- First correct the two demonstrated unknown-as-absence safety defects, with preservation regressions.
- A blocked Git helper, target read, lock holder, or state-storage worker cannot hold a control reply.
- Positive controls complete real work; an unavailable-only responder does not satisfy the goal.
- Native client latency meets the adopted response targets during a real Mini scan and injected stalls.
- Repeated matching requests share one worker/job. Output floods, dead clients and inherited pipes
  cannot cause unbounded capture, worker multiplication or a held client connection.
- Restart, PID reuse, config/binary replacement, lost replies and cancellation never produce false
  completion or stale cleanup authority.
- An unchanged readable fixture has the same candidates with shared discovery and the independent
  reference. New nested declarations remain discoverable. Read failures produce incomplete coverage.
- Three paired native read-only comparisons show lower median CPU and elapsed time for equivalent
  coverage. Include child cost and work counts. Preserve failed and changed-input comparisons.
- The full safety suite and installed-runtime preservation/removal cases pass on both Macs.
  Compare source/help/guides and native signing identity before fleet delivery.

These are implementation acceptance requirements, not results of this investigation.

## Reproduction and retained material

Prepare the diagnostic source copy with `python3 tools/profile_responsiveness.py` in a checkout
without an existing `target/responsiveness-profile`. The tool refuses to overwrite that directory.
Build it with two Cargo jobs, `--release --locked`, release stripping disabled, and debug symbols.
Use `target/responsiveness-build` for its build output.

On the Mini, copy `leases.json`, `reap-files-seen.json`, and `store-bindings.json` where present
into an isolated `XDG_STATE_HOME/reap`. Supply the current config through
`REAP_INTERNAL_CONFIG_SNAPSHOT`. Set `REAP_PROFILE_READ_ONLY=1` only for the diagnostic binary.
Run `files ROOT`, `sweep`, and `stores` sequentially. Capture stderr profile records and stdout.
The original diagnostic deadlines were 900, 600, and 360 seconds respectively.
Stop only the owned dry-run process group if its deadline expires.

Run the isolated blocking controls with
`python3 tools/reproduce_responsiveness.py --destination target/responsiveness-owned-fixtures`.
The destination must be new. The harness verifies preserved data and no remaining owned groups.
It intentionally demonstrates current defects; it is not a passing conformance suite for the
recommended implementation.

Raw native logs, diagnostic binaries, copied state and fixtures remain under
`/Users/spencer/projects/reap/responsiveness-investigation` on the Mini.
That owned target has a sealed KEEP. Its nested lifecycle fixtures do not grant live cleanup
authority once the enclosing declaration is honored. Local regenerable inputs remain under `target/`.
No retained proof depends solely on an ignored Cargo build directory: this report and its
selected receipts are committed project evidence.
