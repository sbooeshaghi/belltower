# TUI Architecture

This document describes the current architecture of `bt-tui`.

It is intentionally based on the implemented system, not an aspirational redesign.
Where the code still carries debt, that debt is called out explicitly.

Related docs:

- [`launcher-and-tui.md`](./launcher-and-tui.md)
- [`../development/tui-testing.md`](../development/tui-testing.md)
- [`../development/tui-reference-review-20260402.md`](../development/tui-reference-review-20260402.md)

## Purpose

`bt-tui` is the operator-facing terminal client for Belltower.

Its job is to make the harness feel like a coherent shell-native product rather than a thin demo wrapper around the API.
That means it has to balance:

- a shell-first terminal model
- append-only transcript history
- a fixed live composer area
- explicit operator controls for session/runtime actions
- truthful visibility into tool use, approvals, and failures

The current direction intentionally follows the strongest parts of Codex, Hermes, and `pi`:

- inline main-screen interaction
- terminal-native scrollback and drag selection
- immutable committed transcript history
- a compact bottom region reserved for the active interaction surface

## Product-Level Invariants

These are the behavioral rules the implementation should preserve.

### Inline Shell Model

- The TUI runs in the primary terminal screen.
- It does not use alternate screen in normal operation.
- It does not capture the mouse.
- Vertical scrolling and drag selection remain terminal-native.

### Append-Only Transcript

- Committed transcript history is append-only.
- Once committed output has been emitted, it is not rewritten for display toggles.
- Historical drill-down happens through `inspect`, not by rerendering older transcript output.
- The live app always renders transcript history in its compact default density; there is no mutable runtime “verbose transcript” state anymore.

### Live Current-Turn Region

- The live region above the composer is for transient UI only.
- In normal operation, streamed assistant text should not live there as a bounded pane.
- Canonical incremental stream output should append into terminal history as it arrives.
- The live region may still contain transient operator context such as:
  - optimistic local user input before canonical commit
  - approval interruption context
  - waiting/reconnect status
  - compact in-flight tool state that has not yet emitted a canonical history representation
- Large operator output, especially `/inspect`, may flush directly into scrollback rather than staying inside the live region.

### Bottom Region Contract

- The composer is bottom-aligned in the default state.
- The composer is a single visible row by default and grows only with actual wrapped input lines.
- When no transient panel is active, the idle layout keeps a quiet inline footer for session, branch, connection, and queue context.
- Once the composer contains draft text, that footer row collapses so the draft and live-turn region stay compact.
- When the composer or transient panel grows and there is still free terminal space below the current inline viewport, that extra height is consumed below the existing shell instead of being inserted above the transcript.
- The terminal scrollback above the shell is only shifted once the expanded inline viewport would otherwise run past the bottom of the terminal.
- The reserved area below the composer is used for exactly one transient interaction surface at a time:
  - slash-command menu
  - approval chooser
  - compact question guidance
- There is no permanently noisy help strip.

### Transient Slash Layout

This is an intentional interaction rule, not an accident of rendering:

- typing `/` opens the command panel below the composer
- opening that panel raises the live bottom region upward
- deleting `/` without executing a command hides the panel and collapses the layout back to the default bottom-aligned state
- executing a slash command also returns the layout to the default bottom-aligned state once the panel closes

### Inspection Over Transcript Mutation

- Tool calls and tool results render compactly in the transcript.
- Non-exploration tools such as write, edit, and shell render as live
  `Calling ...` rows as soon as they are requested, then commit as
  `Called ...` rows when their result arrives.
- Tool summaries include durable `call_id` values.
- Detailed drill-down happens through `inspect`, for both humans and agents.
- Large inspection output must be visible in full, even if that means flushing it into scrollback instead of keeping it in the bounded live region.

### Transcript Presentation Contract

The transcript should use a small, stable visual grammar instead of ad hoc bracket labels.

- user messages render with the composer-style `›` prefix
- assistant prose renders as plain text with preserved paragraph boundaries
- tool activity renders as compact action rows using a `•` prefix
- tool results render as subordinate rows under tool activity using `└`
- wrapped continuation lines align under the visual block they belong to instead of restarting at column zero
- wrapping prefers word boundaries and only falls back to character splits for long unbroken tokens

The intended presentation is closer to Codex than to log-style tagged output.

Examples:

- `› summarize this project`
- `• Calling read src/main.rs · call_123`
- `• Called read src/main.rs · call_123`
- `  └ 214 lines`
- `  └ Error: permission denied`

Important spacing rules:

- there is a paragraph break between a user message and the next assistant/tool block
- consecutive tool rows in the same agent block stay visually contiguous
- there is no blank line between a tool action row and its subordinate result row
- explicit `Thinking` text is only shown before meaningful tool or assistant output becomes visible

## Runtime Model

At runtime the TUI is made of three different presentation layers:

1. terminal scrollback
2. live tail above the composer
3. bottom shell and interaction area

### 1. Terminal Scrollback

Scrollback contains committed transcript history and large operator output that should remain fully visible/selectable.

Examples:

- completed user and assistant turns
- canonical `ask` prompts and recorded answers after the protocol has committed them
- tool calls and tool results after they are committed
- oversized `/inspect` output
- command output that does not fit comfortably in the live region

Scrollback is the durable terminal-facing history surface.

### 2. Live Tail

The live tail is a bounded transient shell surface, not the primary home of agent output.

It exists so the operator can stay oriented during the active turn without losing the shell-first terminal model.

The live tail may include:

- approval interruption context
- compact in-flight tool state when needed
- transient question guidance while the operator is still responding

Streamed assistant output should normally bypass the live tail and append into terminal history in event order.
That is the Codex-style behavior we want: the terminal itself becomes the visible streaming surface, while the live tail stays small and stable near the composer.

### 3. Bottom Shell And Interaction Area

The bottom shell contains:

- a stable spacer row above the shell
- an inline status and queue-preview stack above the composer when the runtime is busy
- the multiline composer
- either the reserved bottom panel or the quiet inline footer

This area is drawn as a stable inline viewport rather than as transcript history.
Its concrete render seam is `BottomSurfaceView`, which yields exactly one of:

- footer
- slash-command menu
- question prompt
- approval chooser

## Event And Reconciliation Model

The hardest part of the TUI is not drawing boxes.
It is maintaining a coherent operator-facing transcript while reconciling:

- optimistic local input
- canonical server events
- streaming deltas
- reconnect catch-up
- transcript page loads and merges

The current implementation keeps `ChatApp` as the protocol/session integrator while splitting local TUI concerns into explicit state owners:

- `ComposerState` owns editable input text, cursor position, history browsing, and composer width
- `BottomShellState` owns transient panel selection, sticky inline-shell geometry, and bottom-panel width bookkeeping
- `ActiveTurnState` owns in-flight turn reconciliation, active tool/action cells, optimistic user state, and stream-controller attachments

Terminal-facing scrollback remains owned by the terminal helpers rather than by those state structs.

### Optimistic To Canonical Flow

When the operator sends a prompt:

1. an optimistic user entry is created locally
2. the server later emits the canonical user `message.appended` event
3. the optimistic entry is reconciled with the canonical event instead of being printed twice, and the canonical entry becomes scrollback-eligible immediately

This same reconciliation must hold across:

- live event-stream delivery
- transcript page merges
- reconnect catch-up

### Streaming Flow

During an active assistant turn:

1. `completion.chunk` deltas are already canonical committed events and should be rendered incrementally in event order
2. streamed assistant text appends directly into terminal scrollback instead of accumulating in a bounded live pane
3. streamed tool previews may also emit an immediate history representation when their first meaningful boundary is known
4. later canonical `message.appended` or tool events must reconcile with what was already emitted, not print older content again somewhere else

The concrete implementation model for this is an ordered active-turn cell:

- the active turn owns ordered transient segments such as assistant text and tool blocks
- a segment may be partially emitted to scrollback while still remaining the current mutable segment
- once a segment portion has been emitted, the TUI never emits that portion again
- canonical `message.appended` events reconcile or finalize existing segments instead of rebuilding transcript output from the full message payload
- when assistant text resumes after a tool block, it starts a fresh assistant segment rather than extending the older tool segment
- finalized assistant prose history keeps the raw markdown source and re-renders from that source at the current width, matching the Codex committed-history seam; tool, ask, operator, and user rows remain specialized transcript cells

The main steady-state path should therefore be:

- committed event arrives
- TUI renders the newly committed fragment
- fragment is appended to terminal history or updates transient bottom-shell state

It should not be:

- rebuild transcript
- clip live text
- replay older content

### Reconnect Flow

The TUI uses event-stream recovery with `Last-Event-ID`.

Important invariants:

- reconnect must not duplicate transcript entries
- stale or already-applied events must be ignored
- reconnect should be visible through the agent-status line
- replay/catch-up must preserve append-only history semantics

### Inspection Flow

Inspection is intentionally not a transcript verbosity toggle.

Instead:

1. compact transcript lines include the persistent `call_id`
2. the operator or agent asks for more detail with `inspect`
3. the detailed output is rendered as a new operator interaction result
4. if that output is too large for the live region, it flushes fully into scrollback

This keeps committed transcript history immutable while still allowing deep drill-down.

## Active-Turn Cell Lifecycle

The live current turn is modeled as one ordered mutable container while the turn is active.

- assistant deltas extend the current assistant segment
- newline-ready assistant text may commit part of that segment into scrollback immediately
- opening a tool call finalizes any current assistant segment and starts a tool segment
- tool completion attaches the result to that same tool segment
- later canonical tool or assistant messages only confirm, extend, or finalize the existing segment sequence
- `turn.finished` clears the live active-turn cell after any remaining uncommitted suffix has been flushed

This is intentionally closer to Codex than to the older bt-tui model that kept separate streamed-id sets and reconstructed assistant remainders from later canonical messages.

## Layout States

The inline TUI can be understood as a small set of layout states.

### Default Bottom-Aligned State

- composer at the bottom
- live tail and shell-top status stack directly above
- no slash menu or approval chooser open

### Raised Slash State

- slash menu occupies the reserved bottom panel
- the visible chat region shifts upward only by the amount of newly reserved panel space
- canceling slash input by deleting `/` collapses this raised state immediately

### Approval State

- approval chooser replaces the slash menu
- the bottom region remains raised
- selection remains active over composer drafts and over a locally in-flight send for the turn that is waiting on approval
- any waiting text is rendered in the shell-top status stack rather than in dedicated footer chrome

### Large Output Flush State

- oversized operator output does not try to remain fully inside the bounded live region
- instead it flushes fully into scrollback and the live region returns to its normal transient role

This is especially important for `/inspect tool <call-id>`.

## Module Ownership

The current code is intentionally split into a small set of functional seams.

### `custom_terminal.rs`

Owns the terminal substrate:

- inline terminal wrapper
- diffed viewport drawing
- viewport height adjustments
- visible clear / hard clear mechanics

This is the lowest-level terminal-facing code in `bt-tui`.

### `inline_terminal.rs`

Owns the inline viewport orchestration:

- desired inline viewport sizing
- scrollback insertion
- processing pending terminal actions
- drawing the transient bottom shell from `BottomSurfaceView`

This is the bridge between terminal substrate and the write-only history model.

### `transcript_view.rs`

Owns transcript projection logic:

- merging transcript entries
- transient live-tail selection
- wrapped-height accounting
- scrollback-eligible text generation

This file is where the append-only transcript model becomes a concrete view model.

### `stream_state.rs`

Owns event-stream and transcript-reconciliation behavior:

- event-stream lifecycle
- reconnect retry scheduling
- streamed event application
- optimistic/canonical user reconciliation
- scrollback queueing and flush inputs
- transcript page merge behavior
- updates to `ActiveTurnState` as canonical chunks, tool calls, and turn-finish events arrive

This module is the most sensitive correctness seam in the TUI after the terminal substrate.

### `session_state.rs`

Owns session-backed refresh and cache maintenance:

- transcript refresh/bootstrap
- session metadata refresh
- readiness/model cache refresh
- MCP cache refresh
- canonical queue inspection refresh plus pending approval/input cache maintenance

This module is the bridge between protocol/session state and the operator-facing UI caches.

### `message_render.rs`

Owns transcript entry rendering policy:

- user and assistant messages
- tool calls and tool results
- diagnostics
- boxed entries
- compact summaries with `call_id`
- streaming preview rendering

This module determines how transcript data becomes user-facing text.

### `command_render.rs`

Owns slash-command result rendering:

- session and status output
- inspection output
- raw/export output
- branch and workflow summaries
- provider/model/MCP output

This keeps structured slash-command output separate from transcript message rendering.

### `command_actions.rs`

Owns slash-command execution and operator/session actions:

- slash-command dispatch
- inspection/export/status action handlers
- operator command recording
- connection/model/tool-mode switching
- branch/session create and resume actions
- approval/deny/cancel/steer actions

This module exists so command behavior can evolve without mixing directly into the event loop or transcript projection code.

### `commands.rs`

Owns slash-command metadata and discovery:

- command registry
- command surface tiers for primary slash commands, hidden advanced slash commands, and contextual-only controls
- parsing helpers
- autocomplete/completion items
- command help output
- one-time startup help output

This is the operator command vocabulary surface. The registry should be the
source of truth for which actions are slash-first, which are contextual, and
which actions are allowed while a turn is active.

### `input_editor.rs`

Owns multiline input editing behavior:

- shell-style text editing
- input history navigation
- slash-command menu navigation
- approval selection navigation
- compact-versus-raised bottom-shell behavior
- table-driven bindings for newline insertion, shell-editing controls, and paste bursts

This module exists because input behavior is a product feature, not incidental widget state.

### `bottom_pane.rs`

Owns the live bottom interaction region:

- quiet inline footer and transient notices
- slash menu rendering
- approval chooser rendering
- one-surface-at-a-time bottom-shell selection via `BottomSurfaceView`
- placeholder policy

This is where bottom-region modes are translated into UI.

### `background_tasks.rs`

Owns background task and recovery orchestration:

- event-stream polling and reconnect scheduling
- session-load completion
- history backfill completion
- send completion
- approval completion

This keeps the async recovery lifecycle separate from both terminal drawing and command initiation.

### `runtime_loop.rs`

Owns the interactive runtime shell around `ChatApp`:

- inline chat bootstrapping
- terminal event handling
- chat-action dispatch
- idle refresh cadence

This keeps the top-level async chat loop out of `main.rs` and makes the event-loop surface much easier to reason about.

### `app_state.rs`

Owns `ChatApp` state helpers:

- local notices and local error recording
- queued input dispatch
- render-cache and transcript-view synchronization
- basic width, follow, and replay state maintenance
- coordination across `ComposerState`, `BottomShellState`, and `ActiveTurnState`

This keeps the mutable application-state behavior together instead of letting it spread back across the crate root.

### `session_loader.rs`

Owns session/bootstrap helpers:

- initial open vs resume selection
- project-root and default-connection resolution
- session/transcript bootstrap loading
- resume candidate selection and sorting
- shared session title and readiness helpers

This keeps startup, resume, and transcript-bootstrap behavior out of the live app/event-loop path.

### `pending_views.rs`

Owns the derived pending-work projections:

- transcript-derived pending non-`ask` tool calls
- compact tool-detail summaries

Canonical pending approvals and pending `ask` requests now come from
`SessionQueueInspection` in `session_state.rs`, while `pending_views.rs` only
derives the remaining transcript-backed pending tool summaries.

### `text_utils.rs`

Owns reusable low-level helpers:

- wrapped cursor math
- character and word movement
- text wrapping
- path/detail truncation
- short-id rendering
- tool-mode parsing and formatting
- stream reconnect delay calculation

These helpers are intentionally dumb and reusable so they do not accrete back into `main.rs`.

### `transcript_helpers.rs`

Owns transcript merge and dedupe helpers:

- optimistic/canonical message merge helpers
- operator-command merge helpers
- operator-command print keys
- local error coalescing helpers

This keeps reconciliation utilities separate from both the live event handlers and the transcript projection layer.

### `main.rs`

Still owns the highest-level application logic:

- shared TUI state types
- send/approval/request initiation
- shared enums, helper structs, and orchestration across the other modules

`main.rs` is now much smaller than the earlier 5-10k-line versions and is down to the shared top-level glue. That is the intended shape, even though some helper types may still deserve narrower homes later.

### `tests.rs`

Owns the `bt-tui` unit test module:

- transcript and reconciliation invariants
- layout and bottom-pane behavior
- command/help/rendering output
- keyboard editing and slash-menu behavior
- regression coverage for the operator-visible bugs we have fixed

Keeping these tests out of `main.rs` materially improves the maintainability of the runtime code without changing test coverage.

## Testing And Invariants

The TUI is already protected by a relatively deep test suite, plus tmux-based manual testing.

The most important invariants to preserve are:

- no duplicate user messages during optimistic/canonical reconciliation
- no duplicate transcript output during reconnect catch-up
- active turn stays visually contiguous near the composer
- large inspect output is visible in full and not clipped
- slash layout raise/cancel/collapse behavior stays stable
- tool and operator output remain in transcript order

The testing guide in [`../development/tui-testing.md`](../development/tui-testing.md) is the source of truth for repeatable manual smoke flows.

## Current Debt

The TUI is much more stable than it was, but it is not “done”.

### `main.rs` Is Still Too Large

`main.rs` still owns too much of:

- send/approval initiation
- session/history control flow
- shared helper types that may still deserve narrower homes

The current module seams are good enough to keep moving, but `main.rs` remains the dominant complexity hotspot.

### Reconciliation Logic Is Subtle

The optimistic/canonical merge rules, transcript page merge behavior, and reconnect catch-up logic are correct enough to dogfood, but they are easy to regress.

This is why the event/reconciliation path needs to stay heavily tested.

### Footer And Bottom-Panel Spacing Need Ongoing Polish

The current layout works, but small regressions in:

- slash-menu height
- footer wrapping
- status-line placement
- live-tail spacing

are still easy to introduce.

That is a product-quality issue, not just a cosmetic one.

### Inspect Output Is Intentionally Split Across Two Surfaces

This is the correct design, but it should stay explicit:

- compact transcript summaries in the live/chat view
- full detail in inspection output, often flushed into scrollback

Trying to collapse these back into one mutable transcript view would reintroduce the same problems that the current design removed.

## Design Direction

The correct near-term direction is:

- keep the shell-first inline model
- keep committed history immutable
- keep `inspect` as the drill-down path
- continue modularizing `main.rs`
- continue tightening tmux/manual smoke coverage for operator-facing regressions

The TUI should continue converging on the best parts of Codex:

- shell-native feel
- compact persistent chrome
- trustworthy transcript behavior
- strong keyboard-first operator control

without losing the Belltower-specific needs around approvals, branches, inspection, and explicit runtime control.
