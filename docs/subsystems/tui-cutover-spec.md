# TUI Cutover Spec

This document defines the inline TUI contract after the Codex-style cutover. It is the rendering and interaction source of truth for `bt-tui`.

## Scope

- One inline shell UI only. There is no supported boxed `Chat / Messages / Input / Context` layout.
- The launcher and `bt-tui` both boot the same inline bottom-shell experience.
- Slash commands, approvals, queued input, session switching, branch switching, inspect flows, provider/model switching, and append-only transcript behavior remain supported.

## Layout Contract

The viewport is a single bottom shell with these regions, top to bottom:

1. transient live tail
2. multiline composer
3. transient panel when active

Committed transcript history remains in normal terminal scrollback above this bottom shell.
New user, assistant, tool, and operator entries should become scrollback-eligible as soon as they are canonically committed.

The composer is fixed to the bottom shell, grows upward, and uses compact Codex-style rendering rather than a bordered text box.

The panel below the composer is reserved for exactly one transient surface at a time:

- slash command menu
- question prompt
- approval chooser

## Visual Rules

- Transcript entries are compact and unboxed.
- User messages render with a `› ` lead line and two-space continuation lines.
- Operator output is compact and readable, not wrapped in diagnostic boxes.
- The composer is unboxed and uses the same `› ` / continuation visual language as user transcript entries.
- Idle layout keeps a quiet inline footer for session, branch, connection, and
  queue context instead of a separate boxed status pane.

## Slash State Machine

Slash layout uses a compact default with a transient raised panel.

- Typing `/` opens the slash command menu below the composer.
- Filtering and autocomplete update as the user types.
- Deleting the slash closes the menu and collapses the layout back to the compact default state immediately.
- Executing a slash command also collapses the layout back to the compact default state once the panel closes.

Question and approval panels replace the slash panel when they become active. Only one transient panel is rendered at a time.

## Composer Behavior

- `Enter` submits the current input unless approval selection is active.
- `Shift+Enter`, `Alt+Enter`, and `Ctrl-J` insert newlines.
- `Ctrl-A`, `Ctrl-E`, `Ctrl-K`, `Ctrl-U`, `Ctrl-W`, `Ctrl-Y`, `Option+Left/Right`, and `Option+Backspace` behave like shell-style editing.
- `Up` and `Down` browse user input history when slash and approval menus are not active.
- Visual wrapping prefers whitespace word boundaries; long unbroken tokens may still split at character boundaries.
- As the composer grows, the bottom shell expands upward by exactly the number of added composer rows.
- Deleting those rows collapses the bottom shell back to the compact default state without ejecting the newest visible chat away from the composer.

## Scrollback Contract

- Older committed transcript entries flush into terminal scrollback.
- Canonical `ask` prompts and recorded answers flush into scrollback as durable history entries; the question panel itself remains transient UI.
- Finalized assistant prose is stored in the TUI history model as markdown source and rendered at the current terminal width, while user prompts, tool rows, ask summaries, and operator commands keep their specialized cells.
- Live tail content only contains the current visible transcript tail, not duplicated boxed history.
- Startup should remain visually clean; guidance should come from the placeholder, `/help`, slash autocomplete, and concise transient notices rather than a permanent help strip.
- When running inside tmux without extended modified-key support, any warning about Option-based editing shortcuts should be transient and should not occupy a permanent footer/status surface.

## Implementation Anchors

The cutover path is currently anchored in these modules:

- `crates/bt-tui/src/input_editor.rs`
- `crates/bt-tui/src/bottom_pane.rs`
- `crates/bt-tui/src/inline_terminal.rs`
- `crates/bt-tui/src/message_render.rs`
- `crates/bt-tui/src/transcript_view.rs`
- `crates/bt-tui/src/session_state.rs`
- `crates/bt-tui/src/command_render.rs`

Old parallel layout helpers should be removed instead of preserved.
