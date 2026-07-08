# Codex TUI Cutover Note (2026-04-07)

This note records the Codex TUI architectural decisions that `bt-tui` should follow during the cutover. The goal is not to imitate Codex superficially. The goal is to transplant the parts of the architecture that make the terminal behavior stable.

## Problem Statement

The old `bt-tui` design mixed incompatible models:

- append-only scrollback
- a transient live tail
- transcript reconstruction from canonical messages
- content-height-driven viewport changes

That combination is unstable. It produces ordering bugs, spacer artifacts, viewport jumps, and full-turn rerenders that are visible even when the final transcript state is correct.

Codex does not use that model.

## Codex Donor Modules

The main donor surface lives under:

- `/tmp/belltower-case-studies/harness/codex/codex-rs/tui/src/markdown_stream.rs`
- `/tmp/belltower-case-studies/harness/codex/codex-rs/tui/src/streaming/mod.rs`
- `/tmp/belltower-case-studies/harness/codex/codex-rs/tui/src/streaming/controller.rs`
- `/tmp/belltower-case-studies/harness/codex/codex-rs/tui/src/streaming/chunking.rs`
- `/tmp/belltower-case-studies/harness/codex/codex-rs/tui/src/streaming/commit_tick.rs`
- `/tmp/belltower-case-studies/harness/codex/codex-rs/tui/src/history_cell.rs`
- `/tmp/belltower-case-studies/harness/codex/codex-rs/tui/src/insert_history.rs`
- `/tmp/belltower-case-studies/harness/codex/codex-rs/tui/src/custom_terminal.rs`
- `/tmp/belltower-case-studies/harness/codex/codex-rs/tui/src/tui.rs`

These files are the architectural source of truth for the cutover.

## Core Codex Decisions

### 1. Committed history is cell-based and append-only

Codex renders committed transcript content as history cells. Once a history cell is inserted above the viewport, it is done. Later canonical messages do not reconstruct already-printed history from transcript state.

Implication for `bt-tui`:

- committed history must remain append-only
- canonical reconciliation cannot repaint already-printed tool calls or assistant text
- transcript rescans are not part of the steady-state rendering path

### 2. There is one mutable active cell

Codex keeps one mutable `active_cell` for the in-flight response phase. That active cell is rendered near the composer and mutates in place until it is ready to commit. This is the key reason the UI looks stable during tool-heavy turns.

Implication for `bt-tui`:

- live rendering should be phase-based, not whole-turn reconstruction
- tool activity belongs in a mutable active container
- the active container should not disappear and reappear as separate transcript fragments during one turn

### 3. Assistant streaming uses a newline-gated markdown collector

Codex does not flush raw tokens directly into history row-by-row. It accumulates deltas in `MarkdownStreamCollector`, renders markdown incrementally, and only commits fully completed logical lines. The unfinished tail remains live until a newline or finalization boundary.

Implication for `bt-tui`:

- assistant streaming must go through a stream collector
- partial lines stay live
- completed lines are committed in order
- paragraph spacing comes from rendered markdown lines, not ad hoc string concatenation

### 4. Streaming drains through a FIFO commit queue

Codex decouples “delta arrived” from “history cell inserted” with a queue and a commit tick. This is what prevents large responses from jumping or repainting whole blocks at once.

Implication for `bt-tui`:

- assistant stream output should be queued, not directly inserted as a rebuilt transcript blob
- commit ordering must follow queue order only
- adaptive catch-up can change drain rate, but not ordering

### 5. Tool activity is phase-local and committed at real boundaries

Codex treats tool activity as active work inside the mutable cell. Tool state changes in place from “calling” to “called” with subordinate result content, then commits as a coherent block.

Implication for `bt-tui`:

- tool call + result should be one logical unit
- later assistant summary must start after the tool block
- canonical tool messages should only reconcile existing live tool state, not create a second transcript rendering path

### 6. History insertion and viewport updates are synchronized

Codex updates the viewport and inserts history inside one synchronized terminal transaction. This matters because clearing, scrolling, and diff redraws are otherwise visible as flicker or overwritten lines.

Implication for `bt-tui`:

- history insertion cannot be treated as an unrelated side effect
- viewport movement and redraw must be coordinated
- multiline composer growth must preserve append-only history above the viewport

### 7. Viewport growth scrolls history; it does not erase it

When the inline viewport grows upward in Codex, the region above it is scrolled upward first. That creates room for the larger viewport without destroying committed history.

Implication for `bt-tui`:

- content-height-driven clear-and-reanchor behavior is wrong
- composer growth must not overwrite scrollback
- terminal scroll/insert operations must be used before reanchoring the viewport

### 8. The bottom shell is a real pane with internal slack

Codex does not collapse the bottom shell to exactly `input_height + footer_height`. The history ends at the top of the bottom shell. Inside that shell:

- the composer starts near the top of the shell
- the footer or popup lives on the bottom rows of the shell
- multiline compose growth happens inside the shell between those two surfaces

This matters because Shift+Enter and other multiline compose operations should consume reserved shell space before forcing viewport growth. That is part of why Codex feels append-only and stable even while the composer grows.

Implication for `bt-tui`:

- history should flow directly into the top of the bottom shell
- the composer and footer should not be flattened into one contiguous block
- multiline compose growth should first use shell slack, not immediately reanchor the viewport

## Bt-Tui Assumptions That Must Stay Deleted

The cutover should keep these assumptions out of the codebase:

- a “live transcript tail” as the primary assistant rendering model
- deferred assistant text paths that hold already-known text for later replay
- transcript reconstruction from canonical `message.appended` bodies
- content-height-driven viewport anchoring that erases history rows
- tests that assert old replay or rerender behavior

## Event Adapter Responsibilities

The belltower-specific layer should stay thin. It should adapt protocol events into the Codex-shaped TUI model:

- `CompletionChunk`
  - feed assistant text deltas into the markdown stream controller
  - open/update live tool state in order when tool deltas occur
- `ToolCallRequested`
  - reconcile or create the active tool cell
- `ToolExecutionFinished`
  - complete the active tool cell and commit it when appropriate
- `MessageAppended`
  - reconcile canonical state only
  - never repaint already-printed content
- `TurnFinished`
  - flush remaining live assistant tail
  - clear only transient live state

## Minimal Behavioral Invariants

The cutover is only successful if these remain true:

- committed history is append-only
- Shift+Enter cannot erase or overwrite prior scrollback
- streamed assistant text appears incrementally in order
- tool calls/results appear where they happen and do not jump later
- the viewport stays anchored above the composer/footer
- multiline compose grows inside the bottom shell before it grows the viewport
- the final end-of-turn state matches what the user saw while it streamed

## Testing Direction

The useful tests after the cutover are:

- streaming prose golden tests
- tool-heavy ordering tests
- terminal history insertion / viewport anchoring tests
- event-to-cell reconciliation tests

The old tests that pinned transcript replay behavior or content-height layout assumptions should stay deleted.
