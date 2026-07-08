# Next Agent Handoff: 2026-04-03

This doc captures the Belltower state at commit `3ef3a59` (`feat(belltower): stabilize prompt stack and inline tui`).
It is historical reference only. The current `bt-tui` implementation now keeps a quiet inline footer in the idle layout; use the current subsystem and development docs for present-tense TUI behavior.

## Scope Of The Committed Work

The committed slice includes:

- managed harness prompt assets under `data/prompts/`
- prompt assembly moved into `bt-context`
- inspectable canonical tool-call drill-down via `inspect`
- provider fixes for empty OpenAI/OpenAI-compatible message content and empty text deltas
- major `bt-tui` modularization:
  - `app_state.rs`
  - `background_tasks.rs`
  - `bottom_pane.rs`
  - `command_actions.rs`
  - `command_render.rs`
  - `commands.rs`
  - `custom_terminal.rs`
  - `inline_terminal.rs`
  - `input_editor.rs`
  - `message_render.rs`
  - `pending_views.rs`
  - `runtime_loop.rs`
  - `session_loader.rs`
  - `session_state.rs`
  - `stream_state.rs`
  - `tests.rs`
  - `text_utils.rs`
  - `transcript_helpers.rs`
  - `transcript_view.rs`
- TUI functional and architecture docs:
  - `docs/development/tui-functional-requirements.md`
  - `docs/subsystems/tui-architecture.md`
  - `docs/subsystems/launcher-and-tui.md`
  - `docs/development/tui-testing.md`

## What Was Verified

These checks passed before the commit:

```bash
cargo test -p bt-tui
cargo test -p bt-providers
cargo check --workspace
```

The `bt-tui` binary is now warning-free in `cargo check --workspace`.

## Current TUI Direction

The intended model is now explicit in the docs and code:

- single inline/main-screen TUI
- no alternate screen in normal use
- terminal-native scrollback and selection
- append-only committed history
- `inspect` for drill-down instead of rerendering history
- bottom-aligned composer with no idle footer reserve
- raised layout only while a transient panel is visibly open or the composer itself has grown:
  - slash command panel
  - multiline composer growth

The live TUI is being pushed toward the Codex model:

- committed history should be inserted above the viewport
- the viewport should remain stable during normal message flow
- only bottom chrome changes should change viewport height

## Updated Status

The old fixed live-viewport plus passive-footer contract is no longer the target.

The current layout direction is:

- append-only committed history in terminal scrollback
- a live current-turn region directly above the composer
- no passive footer reserve in the idle state
- slash/question/approval panels consuming space only while they are actually visible
- multiline composer growth using the same bottom-anchored expansion rule

## Most Likely Root Cause

The current implementation still mixes two concerns:

1. live viewport sizing
2. transcript history insertion

The code path to look at first:

- `crates/bt-tui/src/inline_terminal.rs`
- `crates/bt-tui/src/custom_terminal.rs`
- `crates/bt-tui/src/transcript_view.rs`
- `crates/bt-tui/src/stream_state.rs`
- `crates/bt-tui/src/input_editor.rs`

The remaining work is narrower now:

- confirm live streamed tool previews appear as soon as the model emits tool-call deltas
- keep validating real tmux behavior against the simplified bottom-anchored layout
- continue removing residual assumptions that came from the older sticky footer/viewport model

The Codex reference to study is still:

- `/tmp/belltower-case-studies/codex/codex-rs/tui/src/custom_terminal.rs`
- `/tmp/belltower-case-studies/codex/codex-rs/tui/src/insert_history.rs`
- `/tmp/belltower-case-studies/codex/codex-rs/tui/src/tui.rs`

The key Codex lesson is:

- history insertion and live viewport rendering are separate mechanisms
- the viewport is not resized just because a normal assistant response arrives

## Recommended Next Steps

1. Reproduce the issue in tmux using the freshly built binary.
2. Keep `desired_inline_viewport_height(...)` tied to the actual live tail plus currently visible bottom interaction surfaces.
3. Re-test these scenarios in tmux:
   - clean startup
   - first send
   - second send
   - slash panel open/cancel/send
   - multiline input insert/delete/send
4. Only after the live transcript and tool preview behavior are stable, do any additional polish.

## tmux Repro Recipe

Use the rebuilt binary directly to avoid launcher/helper ambiguity:

```bash
cd /Users/sinabooeshaghi/projects/frollo/belltower
target/debug/belltower chat --connection local
```

Then test:

1. send `hello`
2. wait for the assistant response
3. send `whats up?`
4. type `/` and cancel it
5. insert multiline input with `Shift+Enter` or `Ctrl-J`, then delete it

Capture with:

```bash
tmux capture-pane -pt <session>:<window> -S -120
```

## Important Repo Note

The git repository still contains unrelated root-level changes outside `belltower/`.

Those were intentionally left alone when the Belltower commit was made. Any future commit should stay scoped unless the user explicitly wants the wider repo cleaned up too.
