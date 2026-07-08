# Turn Orchestrator Interface Sketch

This started as the design-before-code artifact for
[`self-hosting-foundation-plan.md`](../planning/self-hosting-foundation-plan.md)
Direction 4.2b. The initial relocation now follows this sketch:
`bt-runtime` owns the turn loop through `TurnOrchestrator`, while
`bt-server` provides provider/auth/tool adapters.

The goal is to make `bt-runtime` the semantic owner of turn orchestration while
keeping `bt-server` as a thin transport and adapter layer.

## Current Problem

`bt-server::run_session_turn_with_turn_id` currently owns more than HTTP
transport. It owns:

- turn-id selection and continuation sequencing
- `TurnStartSource` selection
- settings-revision routing for resumed, queued, and steered turns
- instruction provenance emission ordering
- provider/tool preparation for every turn loop iteration
- completion, tool, failure, budget, and post-turn-control sequencing
- `PostTurnControlAction` matching and loop continuation

That violates the crate ownership rule: runtime orchestration belongs in
`bt-runtime`; route handlers belong in `bt-server`.

## Proposed Runtime Surface

The runtime module is:

```text
crates/bt-runtime/src/turn_orchestrator.rs
```

and export it from `crates/bt-runtime/src/lib.rs`.

The public surface is intentionally small:

```rust
pub struct TurnOrchestrator<'runtime, A> {
    runtime: &'runtime BelltowerRuntime,
    adapters: A,
}

pub struct TurnRunRequest {
    pub session: SessionRecord,
    pub branch: BranchRecord,
    pub initial_turn_id: Option<TurnId>,
    pub initial_turn_started: bool,
    pub initial_settings_revision_id: u64,
    pub initial_source: TurnStartSource,
    pub resumed_from_call_id: Option<ToolCallId>,
}

pub struct TurnRunOutcome {
    pub session_id: SessionId,
    pub final_branch_id: BranchId,
    pub stop_reason: TurnRunStopReason,
}

pub enum TurnRunStopReason {
    Complete,
    AwaitingApproval,
    AwaitingInput,
    BudgetExhausted,
    Cancelled,
    Failed,
}

pub trait TurnExecutionAdapters {
    async fn tool_registry(
        &self,
        session: &SessionRecord,
        branch_id: BranchId,
    ) -> Result<ToolRegistry>;

    async fn runtime_credential(
        &self,
        connection: &ConnectionDescriptor,
    ) -> Result<Credential>;

    fn provider(
        &self,
        connection: &ConnectionDescriptor,
        credential: Credential,
    ) -> Result<ProviderHandle>;
}
```

The exact Rust types may continue to evolve. The invariant is that the runtime
surface receives a typed turn-run request, owns the loop, and uses a small
adapter boundary for dependencies that are not yet runtime-owned.

## Adapter Boundary

The first relocation should avoid a large provider/tool/auth redesign. Keep the
adapter boundary explicit:

- `bt-server` may adapt credential resolution while auth ownership remains
  outside `bt-runtime`.
- `bt-server` may adapt provider construction while provider factories are still
  server-local.
- `bt-server` may adapt tool registry construction because MCP and built-in tool
  registration still depend on server/application state.
- `bt-runtime` owns the ordering and persistence decisions that use those
  dependencies.

This keeps 4.2b focused on orchestration ownership instead of smuggling a
provider, auth, MCP, or tool-registry refactor into the same slice.

## Caller Rewiring

Current callers of `run_session_turn_with_turn_id` are:

- `send_message`: appends the user message, clears direct-message cancel state,
  then starts a fresh user-message turn. After 4.2b it should build a
  `TurnRunRequest` with no initial turn id and
  `initial_source = TurnStartSource::UserMessage`.
- `approve_tool`: resolves the pending approval, records the approval decision,
  executes or denies the approved tool result, persists the tool result, then
  resumes the same turn. In the first relocation this route may still perform
  the approval bootstrap and approved-tool execution in `bt-server`, but the
  continuation loop must move to `TurnOrchestrator`.
- `answer_tool`: records the ask response through runtime bootstrap, then
  resumes the same turn. After 4.2b the continuation loop moves to
  `TurnOrchestrator`; the route keeps request parsing and status-code behavior.
- `run_session_turn`: remains only a thin helper or disappears entirely once
  route handlers call the runtime orchestrator directly.

Approval and ask resume are intentionally called out because their ordering is
easy to break. A resumed turn must not execute provider continuation before the
canonical resumed `turn.started` state is durable or already proven durable by
the bootstrap path.

## What Moves To Runtime

4.2b should move these semantics into `bt-runtime`:

- loop iteration and branch continuation decisions
- `TurnStartSource` and `resumed_from_call_id` propagation
- settings-revision selection through `session_for_settings_revision`
- instruction resolution, plan lookup, context preparation, and instruction
  provenance recording order
- `completion.requested`, `completion.chunk`, `completion.finished`, and
  `turn.finished` ordering
- turn failure persistence and budget checkpointing
- pause detection for pending approvals and asks
- budget-exhaustion stop behavior
- `consume_post_turn_controls` matching for cancel, steer, and queued follow-up

The runtime orchestrator should call existing `BelltowerRuntime` record methods
where possible. New persistence helpers should live in `bt-runtime`, not remain
as hidden server-owned semantic helpers.

## What Stays In Server

`bt-server` should retain:

- route registration and HTTP extractors
- bearer-auth enforcement
- request parsing and response encoding
- HTTP status-code mapping through `ApiError`
- SSE endpoint formatting
- local bootstrap/server state
- adapter implementations for provider construction, credential resolution, and
  tool registry construction until those concerns receive their own design
  slices

`bt-server` should not decide loop continuation, settings-revision semantics,
budget exhaustion behavior, or post-turn control outcomes after 4.2b.

## Commit-Before-Broadcast

The event write path must remain:

```text
typed event -> SQLite commit/seq_id -> event bus/SSE/tracing/export consumers
```

The relocation must preserve 3.4b by ensuring all canonical events emitted from
`TurnOrchestrator` still pass through `BelltowerRuntime` append/record methods.
If any emission site moves, the commit-before-broadcast test follows the new
runtime path rather than accepting a server-local broadcaster.

The orchestrator must not introduce a second event bus, tracing-only canonical
events, or a direct SSE publishing path.

## Implementation Sequence

1. Add `bt-runtime::turn_orchestrator` types and adapter traits with no behavior
   move.
2. Move the server turn loop into `TurnOrchestrator` while preserving existing
   route-level callers and adapter implementations.
3. Move persistence helpers that encode turn semantics from `bt-server` into
   `bt-runtime`.
4. Shrink `bt-server` route handlers to request parsing, runtime calls, and HTTP
   response mapping.
5. Remove any temporary helper that lets server code own loop-control decisions.

Each step should keep tests green and should not widen into MCP lifecycle,
provider factory, auth storage, or TUI changes.

## Verification Gates

Before the relocation PR lands, these tests must already be green:

- `5.4a-min`: representative inspection truth harness
- `5.4a-control`: cancel/steer restart persistence harness
- `3.4b`: commit-before-broadcast

After relocation changes, run:

```bash
cargo fmt --all
cargo test -p bt-runtime
cargo test -p bt-server tests::inspection_contract_min_reconstructs_canonical_session_truth -- --exact
cargo test -p bt-server tests::control_persistence_survives_restart_via_protocol_path -- --exact
cargo test -p bt-server tests::approval_round_trip_executes_tool_and_continues_turn -- --exact
make acceptance
```

`make acceptance` may need an environment that permits local test server binds.

## Risks To Guard

- Do not turn `TurnOrchestrator` into a catch-all runtime god object. It owns
  turn-loop orchestration only.
- Do not make `bt-server` a semantic fallback path for resumed turns.
- Do not reorder approval or ask resume events.
- Do not let queued follow-ups use the current session settings when a queued
  `settings_revision_id` is already durable.
- Do not move MCP lifecycle management into runtime as part of this slice.
- Do not weaken `bt-agent` purity; the agent loop stays storage-agnostic and
  server-agnostic.
