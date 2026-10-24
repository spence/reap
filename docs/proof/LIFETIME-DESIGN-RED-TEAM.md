# Combined lifetime and responsiveness design: adversarial review

Work: `MS-LIFETIME-AUTHORITY.T4`. Date: 2026-10-23.
This records the T4 design. The later [T5 review](CONVERSATION-DESIGN-RED-TEAM.md) covers the
archive-integrated design; its verdict does not replace these historical replies.
Stage: design and source review only. No production code, configuration, lease, policy, service,
or installed guide changed. Native proof below means retained earlier evidence, not a new run.

Design: [DEC-LIFETIME-AUTHORITY](../decisions/DEC-LIFETIME-AUTHORITY.md).
Implementation, conversion, owner ratification, and fleet verification remain open.

## Review method

The parent owns design and acceptance. Two independent, read-only Gateway peers review separate lanes.
Both use Codex `gpt-6.1-sol`, high effort, workspace profile, on Catalyst.
Peer status confirmed those observed selections and native workspace restrictions.
The reviewers use current source, requirements, agent guides, and retained investigation evidence.
They do not run builds, create fixtures, inspect the mini, edit files, or change tracking.

| Lane | Peer | Conversation |
| --- | --- | --- |
| Policy, identity, lifetimes, stores, conversion | `peer-18e1520c9ed7e028-146664` | `conversation-18e1520c9fc87218-146668` |
| Execution, response, callers, concurrency, recovery | `peer-18e15212882be0d0-146855` | `conversation-18e15212894357c8-146859` |

Both first reviews completed against source base `9dd2f30` and the earlier ownership draft.
The second reviews examine the full combined design. Subsequent rounds examine corrections.
Review completion requires the exact correlated terminal reply, not submission or silence.
The stop condition is no remaining material design defect after independent rereview.
Runtime gates are not replaced by review agreement.
The [retained replies](LIFETIME-DESIGN-REVIEWS.json) contain all seven completed review results,
their exact request/reply identities, and observed worker selections. Client credentials are not included.

## Current-use corpus

| Actual workflow or requirement | Required behavior and design coverage |
| --- | --- |
| Agent creates a source worktree, then disappears | Default KEEP survives without renewal; section 4 |
| Interactive workspace awaits human closeout | No inference from silence, merge, build, or agent completion; section 4 |
| Disposable benchmark/log run | Explicit expiry or series retention plus live guards; sections 4 and 6 |
| Preservation copy contains nested expired declarations | Sealed KEEP protects descendants; copied adoption cannot arm deletion; section 5 |
| Same-volume workspace rename | Local policy and native identities travel; scope/ancestor rules remain explicit; section 5 |
| Cross-volume move or raw backup restore | Readable intent, new binding before disposal, journaled preservation; sections 5 and 10 |
| `held-orch`-type legacy scratch tree outside governed roots | No inert-file conversion claim or broad root expansion; exact scope proposal, section 11 |
| Four worker worktrees dominated by ignored Cargo targets | Preserve narrow explicit target permission and unrelated evidence; section 6 |
| Store with two series and one byte cap | Shared cap, per-series newest floor, unconditional minimum age; section 6 |
| Unarmed store or external resource without a binding | Remains inert, never auto-adopted by discovery; sections 6 and 11 |
| Expired parent contains KEEP or unreadable children | Immediate protection and unknown-region closure; sections 5 and 9 |
| Tracked declaration or failing Git probe | Tracked is inert; unknown cannot grant disposal; section 5 |
| Partial cleanup around kept evidence in a checkout | Enclosing and nested Git recovery subjects remain protected; section 6 |
| Agent calls `files PATH` for one workspace | No hidden global scan; policy/sibling closure still required; section 9 |
| Agent runs `create ... && cd ...` | Exit 0 remains completed success; pending is distinct; section 8 |
| Scheduled maintenance overlaps repeated agent requests | One admission path, coalescing, bounded queues, fair units; sections 7–9 |
| Target I/O, state lock, FIFO, probe, or child pipe stalls | Prompt control result, finite resources, no false success; section 7 |
| Coordinator restarts while a worker can still mutate | Reservation and owned process identity survive; section 10 |
| Quarantine, restore, explicit purge, held entry | Durable evidence, preserved selectors/guards, retained restore policy; section 10 |
| Installer leaves an existing operator guide unchanged | Separate live-guide caller migration before activation; section 8 |
| Current macOS and Linux support | Shared policy/protocol; native Mach or abstract-socket transport without synchronous fallback; section 7 |

Sources include `skill/SKILL.md`, the installed Reap and Artifacts skills, `tests/skill_workspaces.py`,
`README.md`, lifecycle tests, and the native investigation.
The current source remains the starting architecture; no review result proves corrected runtime behavior.

## First-round findings and design responses

| Finding or counterexample | Design response |
| --- | --- |
| Copied expiry eventually gains permission after grace | Native directory/declaration binding plus explicit adoption; grace alone never arms a copy |
| New KEEP vanishes during first-seen grace | Protection applies immediately; grace delays disposal permission |
| Unreadable child hides KEEP; failed Git looks untracked | Explicit unknown regions and tri-state Git classification; preservation gates |
| `.reap` Git guard loses all-branch and ignored-data checks | Use stricter retirement recovery subjects and exact Cargo exceptions |
| Partial expiry bypasses the enclosing checkout's guard | Assess enclosing checkout and deep nested repositories; do not split away required Git metadata |
| Store conversion loses minimum age, shared bytes, all-child scope, or disposition | Version 2 retention groups represent each explicitly; exact parity blocks unsupported conversions |
| Conversion leaves old authority active beside new KEEP | Bridge phase journal selects one authority; old executors need verified fences |
| Inert policy outside governed roots is called converted | Exact instance-scoped conversion proposal; no neighboring authority or forced relocation |
| Client or control loop blocks before worker isolation | No target/state/config access, process creation, or synchronous persistence on that path |
| Lost admission reply duplicates mutation | Stable pre-submission key, durable admission, unknown acceptance, idempotent retry |
| U-state causes unlimited replacement workers | Finite slots and admission; unconfirmed exit retains capacity and reservation |
| Global state lock holds all healthy work hostage | Short metadata transactions and durable conflict reservations; common-journal limits remain explicit |
| Service cwd replaces caller cwd protection | Carry caller context and preserve its guard, not the daemon's cwd |
| Queued create/renew/hold looks effective | Exit 0 remains completed success; pending and unknown are distinct |
| Restart or cancellation releases an uncertain mutation | Durable phases, process/reboot identity, safe-unit cancellation, partial-effect outcomes |
| Direct edits race an applying job | Serialized `declare` is the supported concurrent edit; raw edits have an explicit race limit |
| Fast no-op or queue submission is called a performance fix | Real cleanup, coverage parity, paired CPU/elapsed measurements, and installed-fleet gates |

The parent also examines transport lookup, caller-context capture, idempotency compaction, and
state-capacity reservation. Final corrections and rereview outcomes appear below after completion.

## Newly recorded source-derived defects

- `ISS-NEW-KEEP-IN-FIRST-SEEN-GRACE-CAN-LOSE-TO`: `src/files.rs::scan` omits declarations during grace.
  An established parent rule can still select a newly kept child.
- `ISS-PARTIAL-DECLARATION-REMOVAL-CAN-BYPASS-E`: evaluator partial units can omit their enclosing
  checkout from `files` recovery assessment; nested Git discovery also has a three-level cutoff.

These are source-derived traces. No native destructive reproduction or observed data loss is claimed.
They remain open. The two earlier native failure issues also remain open:
`ISS-FAILED-GIT-TRACKING-PROBE-HONORS-A-TRACK` and
`ISS-UNREADABLE-DECLARATION-SUBTREE-HIDES-KEE`.

## Review rounds and final result

### Round 1: requirements and failure traces

Both reviews completed. Correlated request IDs:

- Policy: `cmsg-18e1520cac4cab58-146671`.
- Execution: `cmsg-18e1521295b702c0-146862`.

Their findings produced the full combined draft and the responses above.

### Round 2: full-draft rejection and correction

Both reviews rejected the first full draft with concrete remaining defects.
Correlated request IDs:

- Policy: `cmsg-18e152b899eb1e98-153603`.
- Execution: `cmsg-18e152b8b42b8478-153607`.

| Remaining trace | Correction in the next draft |
| --- | --- |
| Queued adoption first observes a replacement | Require a completed inspection receipt before permissive/adoption submission |
| Persistent file IDs can still permit reuse ambiguity | Require documented non-reuse or effective generation; missing capability blocks adoption and conversion |
| Removing a converted child's old lease removes the legacy parent's nested veto | Every bridge removal applies converted protections as vetoes across authority models |
| Restored file/symlink cannot hold its own KEEP; seals can defeat directory KEEP | Require an effectively kept recovery destination; refuse incompatible paths |
| Old executor resumes after validation despite a new schema | Stable lock and process drain, entry-point switch, concrete type/marker fences, blocked-cohort rule |
| Truncated probe output hides the only open handle | Separate diagnostic truncation from complete safety parsing; incomplete evidence refuses cleanup |
| One worker accumulates millions of entries | Bounded working set, streamed frontier/results, bounded temporary runs, capacity-incomplete protection |
| Lost output loses an automatic request key | Mutation recipes retain a caller-known key before submission; automatic mode has an explicit limit |
| Storage compaction forgets a still-valid key | Durable retired-epoch frontier and whole-epoch compaction; active/uncertain keys remain resolvable |
| Service cannot reconstruct the exited caller's cwd | Worker-side capture and pin before admission, while client remains connected; unavailable capture rejects admission |
| A changed state directory creates a second executor | Explicit namespace context with common physical-resource reservations and worker limits |
| Crash after spawn loses worker ownership | Durable launch intent, child handshake, persisted identity before effects, fixed platform state-writer bootstrap |
| Lane alternation still starves an old accepted job | FIFO within lanes; safe-quantum rotation and reservation release/revalidation |

The parent also replaced pathname-socket lookup with asynchronous Mach-service IPC for the Macs.
That removes target-independent filesystem lookup from the intended response path.
Neither the transport choice nor a written latency target is native performance proof.

### Round 3: correction review

Both reviewers reread the corrected full draft and reported no remaining blocking or high finding
in their lanes. Correlated request IDs:

- Policy: `cmsg-18e15351e18da588-159093`.
- Execution: `cmsg-18e15351f18b7528-159097`.

The policy reviewer confirmed inspection-bound adoption, mandatory identity capability,
cross-authority vetoes, and effective restore retention.
The execution reviewer confirmed admission, resource bounds, caller capture, launch recovery,
fairness, and the concrete old-executor barrier.

### Linux follow-up

The parent confirmed existing Linux support in `README.md` after the full-draft review.
The execution reviewer examined the proposed Linux transport/lifecycle addition separately.
Request `cmsg-18e153771734f7e8-160732` completed with no material finding in that bounded addition.
Abstract addressing does not excuse slow connection or activation from the response budget.
An isolated namespace must return unavailable, not create another applying broker over shared targets.
The final draft includes that reviewed addition and a creation-preflight clarification of existing
scope and seal rules. The parent owns acceptance of the combined result.

## Parent acceptance and limits

The design stage is complete. Each current requirement has a stated mechanism or an explicit
fail-closed capability boundary. All material review findings have a design correction.
There is no known unresolved blocking design finding from these reviews.

**First implementation gate:** prove adequate non-reuse or object-generation capability on the actual
fleet filesystems. That proof does not exist in this design work.
Remount persistence, writable timestamps, UUIDs, and a few successful copy examples cannot replace it.
Missing capability blocks disposal adoption and migration; there is no weaker automatic fallback.

Other explicit limits remain:

- Same-volume moves preserve binding only within effective scope and ancestor policy.
- Raw cross-volume moves need adoption; exact out-of-root scopes can need an explicit update.
- Direct file edits are not serialized against a mutation after its final policy read.
- A stalled volume or common journal can stop useful work without stopping control replies.
- Capacity exhaustion can leave coverage incomplete; dependent data remains protected.
- Lost automatic-key output requires reconciliation, not a fresh-key mutation retry.
- Compatibility fences cover supported entry points and known versions, not arbitrary old programs.

Required verification for this stage: exact completed review replies, local link resolution, valid
review JSON, and `git diff --check`. These checks passed on the authored documents.
No Rust code changed. No build, runtime timing, migration, cleanup, or fleet acceptance is claimed.
The implementation gates in section 12 remain open. Owner ratification remains T2, not this task.

Both owned review conversations and idle workers are closed at handoff.
