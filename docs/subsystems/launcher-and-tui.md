# Launcher and TUI

This document covers:

- `belltower`
- `bt-tui`
- the operator-facing parts of `bt-client`

For the concrete `bt-tui` design and module ownership, see [`tui-architecture.md`](./tui-architecture.md).

The closest analogues are the `pi` coding-agent UI docs, the `pi-tui` README, and Hermes's first-run/operator experience.

## Purpose

This is the user-facing surface of Belltower.

The launcher and TUI are responsible for making the harness feel coherent:

- first-run setup
- auth and model readiness
- session creation and resume
- branch and approval visibility
- live streaming and tool visibility

## Launcher Responsibilities

The `belltower` command should:

- be the default product entrypoint
- run setup when needed
- surface login/model/doctor/status coherently
- auto-start the local server when appropriate
- launch the TUI through the normal protocol path
- run headless one-shot prompts through the same protocol/runtime/session path

This is one of the strongest Hermes patterns worth copying directly.

In a workspace checkout, the launcher should prioritize a fast, truthful dev loop:

- reuse already-built workspace helper binaries when they are available and intentionally built
- preserve an explicit fresh-helper path when the operator needs to force current source into the run
- never hide which path is being used

The current operator contract is:

- `cargo run -p belltower --` should reuse built `bt-server` and `bt-tui` helpers if they already exist in the workspace target directory
- workspace helper reuse is only valid when the helper binary is newer than
  the helper crate and its local path dependencies; rebuilding the launcher
  alone must not force slow Cargo helper startup
- `cargo run -p belltower -- chat --fresh-helpers` should force the fresh helper path through Cargo when the operator explicitly wants current source over speed
- `doctor` should say which helper path will be used by default
- reused local servers are accepted only after an authenticated compatibility
  probe; freshly spawned workspace servers wait for the listener to accept
  connections, which can only happen after the server has written its
  endpoint-scoped auth token and bound the socket
- fresh chat startup probes readiness for only the selected connection, plus
  `local` when implicit fallback is needed; full multi-connection inspection
  remains available through `doctor`, `/status`, and readiness/model inventory
  surfaces instead of blocking first launch
- if the implicit default remote connection is unavailable but `local` is ready, the launcher may fall back to `local`; explicit `--connection` selections still fail closed and report the structured readiness reason
- `belltower run --prompt ...` is the canonical headless launcher path for scripts and benchmarks; it shares startup/readiness/server bootstrapping with `belltower chat`, then uses `bt-client` to create a normal session and send a normal user message

## TUI Expectations

The TUI should be excellent, not merely serviceable.

At minimum it should support:

- creating or resuming a session
- command-style input for session controls such as provider/model switching
- explicit persistence of future-session defaults without mutating the current session
- streaming assistant output
- message queueing while a turn is in flight
- interrupt and redirect controls while a turn is in flight
- terminal-native scrollback and selection
- visible tool activity
- obvious approval actions
- branch visibility and switching
- branch creation from the interactive flow
- model/provider/MCP status visibility
- export access

## Functional Contract

The current operator direction is intentionally compact and shell-first, and should continue converging on the best parts of Codex and Hermes.

### Core Interaction Model

- The TUI is a single inline, main-screen interface.
- It must not use alternate screen in normal operation.
- It must not capture the mouse, so wheel scrolling and drag selection remain terminal-native.
- It should feel like a shell-native harness, not a fullscreen pane app.

### Transcript Model

- Committed transcript history is append-only.
- Once transcript output is committed, it is not rerendered or mutated in place.
- Historical detail is recovered through inspection, not through replay-based verbosity toggles.
- Explicit transcript replay is reserved for session bootstrap, resume, reconnect catch-up after reset, or similar state recovery paths.

### Live Turn Region

- Transcript output is emitted into normal terminal scrollback above a fixed live bottom region.
- The live region above the composer should keep the active turn contiguous near the prompt.
- The live region may include the newest committed user or assistant entry when needed to keep the current turn readable.
- Canonical `ask` prompts and recorded answers are part of durable transcript history; only the question panel itself is transient.
- Older committed history should move into scrollback.
- Large command output, especially inspection output, may flush directly into scrollback instead of staying fully in the live region.

### Composer

- The composer is fixed at the bottom of the terminal.
- It is two visible rows by default and grows vertically only when the typed draft or pasted content actually wraps past that compact baseline.
- It supports shell-style editing:
  - `Shift+Enter` and `Ctrl-J` for newline insertion
  - `Option+Left` and `Option+Right` for word movement
  - `Option+Backspace` for deleting the previous word
  - `Ctrl-A`, `Ctrl-E`, `Ctrl-K`, `Ctrl-U`, `Ctrl-W`, and `Ctrl-Y`
  - `Up` and `Down` for prior user-message history
  - multiline paste bursts without accidental early submit
- It shows ghost placeholder text when empty.

### Slash Command Layout Behavior

This behavior should explicitly match Codex-style interaction.

- In the default state, the composer alone is bottom-aligned.
- Conversation history sits above them.
- Typing `/` opens the command panel below the composer.
- Opening the command panel appends panel space below the composer and pushes the visible live chat region upward by exactly that panel height.
- If the user cancels slash input by deleting `/`, the command panel disappears and the layout collapses back to the default bottom-aligned state immediately.
- If the user executes a slash command, the layout also returns to the compact default state once the panel closes.
- This compact-versus-raised behavior should be stable, not incidental.

### Reserved Bottom Area

- The reserved area under the composer behaves in one of three modes:
  - slash-command selector
  - approval chooser
  - compact question guidance
- The slash-command panel is keyboard-first, compact, and stable while typing.
- The command menu should render as concise bounded single-line entries rather than relying on awkward paragraph wrapping.
- The approval chooser should be obvious, arrow-navigable, and confirm with `Enter`.

### Footer And Status

- The default empty-composer layout keeps a quiet inline footer under the composer for session, branch, connection, and queue context.
- Once the composer has draft text, that footer row collapses instead of reserving extra space under the draft.
- Waiting state text such as reconnecting, working, waiting on approval, or queued follow-up preview should render as part of the shell-top status stack directly above the composer.
- Once streamed tool or assistant output is visible in the live turn region, waiting-state text should not duplicate that busy state elsewhere.

### Startup Behavior

- The visible pane should start clean on first load.
- Operator guidance should come from the placeholder, `/help`, slash autocomplete, and concise transient notices instead of a permanently occupied help strip.
- After a session is created or selected, the TUI should paint the shell before
  session metadata refresh, readiness refresh, MCP refresh, or SSE catch-up
  work. Those enrichments must still use the protocol path, but they should not
  block the operator from seeing that Belltower is live.
- Readiness/model inventory refresh is intentionally excluded from the startup
  critical path; commands that need that view refresh it on demand. The TUI
  opens without fetching every connection's model inventory. The targeted
  `/models <connection>` command refreshes one connection, while `/models`,
  `/status`, and `/doctor` remain explicit full-state operator checks.
- When the configured default connection is remote and the operator did not
  explicitly request it, launcher startup uses a short bounded readiness probe.
  If that probe is missing, degraded, unreachable, or slow while `local` is
  ready, startup falls back to `local` and leaves full remote readiness to the
  post-paint TUI/status surfaces. Explicit `--connection` remains strict.
- Creating a fresh chat session should take the narrow creation path and avoid
  listing or hydrating unrelated historical sessions; resume and explicit
  session selection remain the paths that need session-list reads.
- Startup timing diagnostics are opt-in via `BELLTOWER_STARTUP_TRACE`. Set it
  to a file path, for example
  `BELLTOWER_STARTUP_TRACE=/tmp/belltower-startup.log cargo run -p belltower --`,
  to collect launcher, server, and TUI process milestones without writing
  diagnostics into terminal history, session history, or canonical telemetry.

### Tool Visibility And Inspection

- Tool calls and tool results render compactly in the transcript.
- Streamed tool activity should appear in the live turn region as soon as the model emits tool-call deltas, not only after the committed tool-call message lands.
- Tool summaries include persistent `call_id` values for later drill-down.
- Tool errors render clearly and durably in transcript order.
- `/inspect tool <call-id>` drills into a specific tool call in detail without rerendering prior transcript output.
- Humans and agents should share the same inspection concept and data model.

### Streaming And Reconnect

- Streaming assistant text should remain visually near the composer and active turn.
- The transition from live streaming to committed output should be stable and predictable.
- Explicit reconnect status should appear in the agent row.
- Event-stream reconnect uses `Last-Event-ID` recovery and must not duplicate transcript entries.

### In-Flight Controls

- The default slash surface is intentionally narrow and centered on frequent operator flows:
  - `/help`, `/use`, `/status`, `/defaults`, `/session`, `/inspect`, `/history`, `/usage`, `/new`, `/branch`, `/cancel`, `/mcp`, `/models [connection]`, `/connection`, and `/compact`
- `/connection` and `/use` are the primary session-local provider/model controls.
- `/use` commits the session-local connection/model through the server
  settings path and returns immediately from that durable mutation. It must
  not synchronously block on full provider readiness or remote model inventory;
  `/status`, `/models`, and `/doctor` are the explicit readiness/model
  inspection surfaces.
- `/defaults` is the global-defaults surface for future sessions and writes `~/.config/belltower/config.toml`.
- persisted default models should be validated against the canonical discoverable model inventory for that connection when discovery is available, rather than accepting arbitrary catalog or typed model ids.
- Approval resolution is contextual rather than slash-first:
  - approve/deny choices are driven from the approval chooser when a tool call pauses
- Queue state is contextual rather than slash-first:
  - queued follow-ups should be visible in the normal control-plane flow instead of through a dedicated top-level slash command
- Advanced or still-unsettled commands may remain exact-typed but should not dominate the default slash menu or default `/help` output.
- Explicit in-flight turn exit semantics are:
  - `Esc` warns
  - `/detach` leaves the session running
  - `Ctrl-C` cancels the active turn through the same control path as `/cancel`

### Non-Goals

- no alternate-screen focus mode as the primary design
- no mutable historical transcript rendering
- no mouse-owned transcript scrolling that breaks terminal-native selection
- no permanently verbose operator chrome

The strongest external patterns informing this are:

- from Hermes: a coherent operator story around readiness, provider/model state, and approval scopes, delivered in a main-screen shell interaction model
- from Codex: an inline terminal viewport model where transcript and composer cooperate with terminal scrollback instead of replacing it

Current `bt-tui` code ownership is intentionally split along those same seams:

- [`crates/bt-tui/src/custom_terminal.rs`](../../crates/bt-tui/src/custom_terminal.rs) owns the custom inline terminal substrate, diffed viewport drawing, and the Codex-style viewport growth rule that consumes free rows below the shell before scrolling transcript history
- [`crates/bt-tui/src/inline_terminal.rs`](../../crates/bt-tui/src/inline_terminal.rs) owns inline history insertion, viewport sizing, and the live-only bottom draw path
- [`crates/bt-tui/src/app_state.rs`](../../crates/bt-tui/src/app_state.rs) owns `ChatApp` state helpers plus the coordination layer across `ComposerState`, `BottomShellState`, and `ActiveTurnState`
- [`crates/bt-tui/src/bottom_pane.rs`](../../crates/bt-tui/src/bottom_pane.rs) owns command/approval/question panels, the inline footer, and the one-surface-at-a-time `BottomSurfaceView` seam
- [`crates/bt-tui/src/command_render.rs`](../../crates/bt-tui/src/command_render.rs) owns slash-command, inspection, session-summary, and raw/export output formatting
- [`crates/bt-tui/src/command_actions.rs`](../../crates/bt-tui/src/command_actions.rs) owns slash-command execution, operator command recording, and session/branch action handlers
- [`crates/bt-tui/src/runtime_loop.rs`](../../crates/bt-tui/src/runtime_loop.rs) owns the interactive chat loop, terminal event handling, chat-action dispatch, and idle refresh cadence
- [`crates/bt-tui/src/message_render.rs`](../../crates/bt-tui/src/message_render.rs) owns transcript formatting policy for user/assistant/tool/error/structured output and live delta previews
- [`crates/bt-tui/src/pending_views.rs`](../../crates/bt-tui/src/pending_views.rs) owns transcript-derived pending tool projections and compact tool-detail summaries for non-`ask` tools
- [`crates/bt-tui/src/stream_state.rs`](../../crates/bt-tui/src/stream_state.rs) owns event-stream lifecycle, optimistic/canonical reconciliation, and scrollback queueing
- [`crates/bt-tui/src/background_tasks.rs`](../../crates/bt-tui/src/background_tasks.rs) owns background task polling for reconnect, session-load completion, history backfill, sends, and approval resolution
- [`crates/bt-tui/src/session_loader.rs`](../../crates/bt-tui/src/session_loader.rs) owns session-open/resume selection, transcript bootstrap/loading, and session-title/readiness helpers
- [`crates/bt-tui/src/session_state.rs`](../../crates/bt-tui/src/session_state.rs) owns transcript refresh, metadata/readiness/MCP refresh, canonical queue inspection refresh, and pending approval/input cache maintenance
- [`crates/bt-tui/src/text_utils.rs`](../../crates/bt-tui/src/text_utils.rs) owns reusable text wrapping, cursor movement, short-id rendering, tool-mode parsing, and truncation helpers
- [`crates/bt-tui/src/transcript_helpers.rs`](../../crates/bt-tui/src/transcript_helpers.rs) owns transcript merge helpers, operator-command dedupe keys, and local error coalescing helpers
- [`crates/bt-tui/src/transcript_view.rs`](../../crates/bt-tui/src/transcript_view.rs) owns transcript projection, immutable tail selection, and scrollback insertion inputs
- [`crates/bt-tui/src/input_editor.rs`](../../crates/bt-tui/src/input_editor.rs) owns multiline composer editing, input history browsing, slash-menu navigation behavior, and the table-driven shell/editing keybindings
- [`crates/bt-tui/src/tests.rs`](../../crates/bt-tui/src/tests.rs) now owns the unit test module that previously lived inline in `main.rs`
- [`crates/bt-tui/src/main.rs`](../../crates/bt-tui/src/main.rs) now primarily owns shared types/constants, including the extracted composer/bottom-shell/active-turn state structs, and the remaining top-level glue across the extracted modules

For the fuller runtime model, reconciliation flow, and open debt in this subsystem, see [`tui-architecture.md`](./tui-architecture.md).

The best practical lessons here are:

- tree/session navigation matters
- the operator needs visible context and status
- interrupt and redirect semantics must be explicit
- terminal ergonomics are a product feature, not an afterthought

For a more current tmux-based comparison against installed `codex`, `claude`, `hermes`, and `pi`, see [`development/tui-reference-review-20260402.md`](../development/tui-reference-review-20260402.md).

## Interaction Model

The TUI must consume the same protocol as any future GUI.

That means:

- no hidden in-process runtime shortcuts in v1
- session state comes from server/client behavior
- control actions like approvals and branch switches are protocol operations, not local-only hacks
- message submission outcome comes from the server, not local TUI guesswork
- launcher readiness, setup follow-up checks, and `doctor` output should render from the shared `bt-readiness` inspection surface rather than maintaining a second launcher-local probing contract
- readiness ordering, labels, and blocked-vs-ready decisions should come from structured readiness state in shared inspection DTOs, not from local string parsing or ad hoc `probe_ready` heuristics
- launcher-owned local bootstrap behavior should stay distinct from the baseline remote-client contract; see [`../development/consumer-modes.md`](../development/consumer-modes.md)

For direct message sends specifically:

- `POST /sessions/{session_id}/message` returns a structured canonical outcome
- `dispatched` means the server accepted the message and ran the turn immediately
- `queued { position }` means the server persisted the follow-up in the session queue instead
- launcher and TUI surfaces should react to that response instead of inferring queueing from local busy heuristics alone
- pending approvals, pending asks, queued follow-ups, cancel state, and pending steers should be rendered from `SessionQueueInspection` or equivalent canonical control surfaces, not transcript rescans alone

## TUI Testing

TUI testing should include a fixed-size `tmux` workflow and explicit checks for shell-native interaction.

Example:

```bash
tmux new-session -d -s belltower-test -x 100 -y 30
tmux send-keys -t belltower-test "cd /Users/sinabooeshaghi/projects/frollo/belltower && cargo run -p belltower --" Enter
sleep 3 && tmux capture-pane -t belltower-test -p
tmux send-keys -t belltower-test "hello" Enter
tmux kill-session -t belltower-test
```

This should become an official testing guide after the TUI slice lands more fully.

## Human Checkpoints

1. run `belltower`
2. create or resume a session
3. send a prompt and watch streaming output
4. trigger a tool call and resolve an approval
5. interrupt or steer a running turn
6. use terminal scrollback and drag selection on the transcript
7. inspect branch/session visibility in the UI

## Current Priorities

The next high-value work here is:

- continued polish of the append-only inline history model, reconnect UX, and tmux behavior under long-session smoke tests
- small operator refinements on top of `/status`, `/models`, `/doctor`, `/mcp`, and `/use`
- keeping launcher and TUI readiness/model surfaces coherent as the model registry and MCP surfaces evolve
- faster repeated startup from a workspace checkout without sacrificing binary-truthfulness
