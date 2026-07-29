# Belltower Dogfood Log — 2026-07-29 (Day 1)

## Gate status

**M9 remains open and the `0.1.0` release remains NO-GO.** This is one
honest day of Belltower-on-Belltower evidence, not the required operator week
(5 business days). It was run from dirty integrated worktree HEAD
`478c5cfea8b925af0fc5fb6dbe96a3b6e95fc064`, not the clean immutable release
candidate. No earlier date is backfilled by this entry.

The canonical event log is the source of truth for the facts below. TUI text
is treated as an operator view and is cross-checked against the SQLite event
store and derived projections.

## Date, providers, and sessions

Date worked: **2026-07-29**. The TUI was the only Belltower operator
interface. The launcher command was:

```text
target/debug/belltower chat --connection chatgpt --project-root /Users/sinabooeshaghi/projects/frollo/belltower --no-setup
```

| Role | Session | Branch | Provider / model | Outcome |
| --- | --- | --- | --- | --- |
| Parent | `c7c47dbd-4e7a-4358-bafd-dd9096d35bf6` | `53aa8529-1ae5-4deb-814a-3aa61fc90730`, then `0bb3ff0e-c820-4ef1-b0d1-9541731ff04a` | ChatGPT / resolved `gpt-5.4-mini` | Completed release audit, coordination wake handling, and code/test review |
| Local child | `b524702e-0932-4caa-9191-8a93c1ff1229` | `73fd428b-686a-46d5-800f-91387b4b1c26` | local OpenAI-compatible / `qwen3.5:latest` | Delivered a progress result; two long/queued turns were explicitly cancelled |
| Cloud child | `85aed0fc-59bd-4f03-8df5-ee3135b32b52` | `61b0a283-dc64-4550-8f41-7702d806e325` | ChatGPT / `gpt-5.6-sol` | Completed bounded-coordination audit and sibling/parent messaging |

The final `/workflow` view reported all three sessions idle, zero pending
approvals, and zero outstanding cancel requests. `/detach` then exited the TUI
and launcher; the tmux server no longer existed.

## Real repository tasks attempted today

This list is Day 1 evidence, not a claim that the full-week or five-task M9
exit bar is satisfied.

1. **Release-requirement audit — completed.** The parent read IR-15/M9,
   foundation plan section 5.7, and the dated release-readiness review, then
   reported the exact remaining operational evidence and NO-GO status.
2. **Canonical event-family audit — completed-with-friction.** The local child
   read the event-taxonomy and source-of-truth documents, used `list_agents`,
   and sent the parent message `74d69d63-9396-4de9-8f46-0c9bf8e2a650` with a
   useful mapping of spawn, messaging, branch, compaction, tool, inspection,
   and export evidence. Slow local completion and a queued sibling wake
   required two explicit `/cancel` operations.
3. **Subagent coordination-contract audit — completed.** The cloud child read
   the subagent/session-graph and participant/collaboration documents, used
   `list_agents`, sent sibling question
   `96909afc-65bc-4b1d-9e87-c1e59313deca`, and sent parent result
   `777afe72-0ba5-4315-b279-edb4ab598a12`. The parent was durably woken and
   processed the result.
4. **Post-refactor cancellation review and targeted test — completed-with-friction.**
   The parent reviewed `runtime/cancellation.rs`, `control_plane.rs`, and the
   live `turn_orchestrator.rs` call site. It found the canonical cancel check
   and tool-request admission remain serialized under the store lock. File
   counts were 262 and 1388 lines. The exact command below passed with 1 test,
   0 failures, and 85 filtered tests:

   ```text
   cargo test -p bt-runtime canonical_cancel_wins_before_tool_request_and_prevents_execution_admission --locked
   ```

5. **Operator state-management and telemetry exercise — completed.** The TUI
   created and activated a branch, inspected the session, manually compacted
   context, exported JSONL and OTLP JSON, exercised approval, inspected the
   heterogeneous workflow, cancelled live local-model work, and detached.
   This is a release-validation task; it is not counted here as proof of a
   fifth distinct coding task.

## Canonical assertions

- Parent spawn events: 2 `session.spawn.requested` and 2 `session.spawned`.
- Across the three sessions: 18 `session.related_message.recorded`, 8
  `session.related_message.resolved`, and 3
  `session.related_message.settled` events.
- Cloud-to-local sibling question
  `96909afc-65bc-4b1d-9e87-c1e59313deca` and cloud-to-parent result
  `777afe72-0ba5-4315-b279-edb4ab598a12` have paired sent/received records
  with exact source and destination branches.
- Branch creation, summary, and activation are canonical at seq 146132,
  146133, and 146136. The child branch is
  `0bb3ff0e-c820-4ef1-b0d1-9541731ff04a`.
- Manual compaction is canonical at seq 146876: forced/completed with
  `gpt-5.4-mini`, 30 to 30 messages, and 11166 to 11148 estimated tokens.
- The targeted test's successful shell call is
  `call_n8twhjrljcgSH6R0eszYJfxV`: status 0, `timed_out=false`, 1 passed.
  A preceding request incorrectly asked for 1200 seconds and was rejected by
  the configured 120-second maximum; a 120-second compilation attempt then
  timed out before the successful retry.
- Cancellation events at seq 152936 and 153313 are each followed by a single
  cancelled `turn.finished`; both cancel requests are later cleared. The two
  local turns have canonical terminal status `cancelled`.
- The final aggregate event ranges are parent 137908–154300 (7651 events),
  local child 141283–154472 (2627), and cloud child 141540–154035 (2875).

## Export evidence

The TUI wrote and validated temporary local artifacts (removed after the
assertions below were recorded):

- `/tmp/belltower-dogfood-20260729.jsonl`: 2,414,997 bytes and 4038 valid JSON
  records (seq 137908–146877), all for the parent session, with unique event
  IDs. This snapshot predates the later code-review/cancel events.
- `/tmp/belltower-dogfood-20260729-otlp.json`: 15,134,350 bytes; `jq` validated
  a non-empty `resourceSpans` array with Belltower session/project resource
  attributes.

The OTLP file validates serialization only. It was **not** ingested by Phoenix
or another live OTLP consumer, so that release assertion remains incomplete.

## TUI friction

- **Approval prompt UX:** the mid-turn shell prompt clearly offered approve
  once/session/always/deny, and Enter approved once; no prompt re-entry issue
  was observed after scrollback.
- **Long tool-output rendering:** previews and transcript output used visible
  truncation markers and remained navigable; `/inspect` preserved exact
  high-level counts, but the very large export preview was still noisy.
- **Slash-command discoverability:** `/spawn`, `/workflow`, `/branch`,
  `/compact`, `/export`, `/resume`, `/cancel`, and `/detach` worked. Typing
  `/inspect` first opened command completion and required an extra selection
  and Enter, which was minor discoverability friction.
- **Error rendering:** a shell timeout appeared in the transcript as a
  truncated JSON error and was not actionable without canonical inspection;
  provider/auth/budget errors were not exercised today.
- **Session controls:** `/compact`, `/inspect`, and `/cancel` were used during
  real work. `/cancel` durably terminated the current local turn, but a pending
  sibling wake immediately opened another turn, requiring a second `/cancel`.
  `/steer` was not exercised today, so its dogfood assertion is incomplete.
- **First-contact bootstrap:** launch showed the exact session, branch,
  connection/model, project root, and `/help` hint; no bootstrap ambiguity was
  observed with the intentionally supplied `--no-setup` flag.

## Unfiled friction issue candidates

No GitHub issues were created because this run was not authorized for external
writes. These remain unfiled candidates; therefore the required linked issue
list and week-end triage are incomplete.

- **Direction 5 — error rendering:** show full actionable cause for tool
  timeout/configuration errors without requiring SQLite inspection.
- **Direction 5 — queued control UX:** make it obvious that `/cancel` targets
  only the current turn and that a queued related-session wake may start the
  next turn immediately.
- **Direction 5 — local-model latency:** surface cold-start/generation progress
  more clearly during a roughly 10-minute local child turn.
- **Direction 5 — slash discoverability:** reduce the extra confirmation needed
  to reach `/inspect session`.
- **Direction 1 / Direction 5 — projection clarity:** the historical turn
  `51ceb30f-13f2-4d5a-9b9c-c5dcd210599a` remained projected as
  `awaiting_approval` even though its approval was resolved and the final
  workflow showed zero pending approvals; determine whether this is intended
  historical state or a stale projection.

## Historical evidence audit and remaining gate

No prior `dogfood-log-*.md` exists. Read-only inspection found older
Belltower-root sessions, but the candidate multi-model runs were toy prompts
or temporary smoke roots and lacked the contemporaneous five-day task,
friction, and mandatory TUI record required by section 5.7. They are not
backfilled as qualifying evidence.

Remaining before M9/release GO: four more business days; enough additional
real coding tasks to satisfy the minimum honestly; `/steer` plus missing error
classes under real work; authorized linked issue filing and week-end triage;
an external collaborator/install handoff; a live OTLP consumer; the five-target
release-candidate matrix; and a rerun of all required evidence from the exact
clean SHA that will be tagged.

**Triage verdict:** foundation is not ready to tag; Day 1 demonstrates the
core self-hosted, heterogeneous-agent, messaging, branch, compaction,
inspection, cancellation, test, and export paths, but M9 and immutable release
evidence remain incomplete as listed above.
