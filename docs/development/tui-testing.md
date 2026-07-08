# TUI Testing Guide

Belltower's TUI is part of the product surface, not a convenience wrapper.
Use this guide for repeatable manual testing as the TUI becomes richer.

Related docs:

- [`../subsystems/launcher-and-tui.md`](../subsystems/launcher-and-tui.md)
- [`../planning/target-implementation-goals.md`](../planning/target-implementation-goals.md)
- [`../../AGENTS.md`](../../AGENTS.md)

## 1. Why `tmux`

Fixed-size `tmux` sessions make TUI regressions reproducible:

- layout overflow
- broken redraws
- live-tail or transient-panel truncation
- keybinding regressions
- approval or branch-navigation rendering bugs
- inline scrollback ordering and append-only history behavior

## 2. Standard Session

Use a stable terminal size:

Important development note:

- when testing from a workspace checkout, prefer `cargo run -p belltower --`
- for a fast repeated dev loop, build the helpers intentionally once, then let `belltower` reuse them
- use `--fresh-helpers` when you explicitly want the launcher to force a fresh Cargo-backed helper run

Recommended preparation:

```bash
cargo build --bins
```

```bash
tmux new-session -d -s belltower-test -x 100 -y 30
tmux send-keys -t belltower-test "cd /Users/sinabooeshaghi/projects/frollo/belltower && cargo run -p belltower --" Enter
sleep 3
tmux capture-pane -t belltower-test -p
```

Expected startup shape:

- prior terminal scrollback remains available above the inline shell
- the idle state keeps the composer at the bottom with a quiet inline footer for session/context details, not a separate boxed status row

To force the fresh helper path:

```bash
tmux new-session -d -s belltower-test-fresh -x 100 -y 30
tmux send-keys -t belltower-test-fresh "cd /Users/sinabooeshaghi/projects/frollo/belltower && cargo run -p belltower -- chat --fresh-helpers" Enter
sleep 3
tmux capture-pane -t belltower-test-fresh -p
```

To stop:

```bash
tmux kill-session -t belltower-test
```

## 3. Sending Input

Examples:

```bash
tmux send-keys -t belltower-test "Summarize the current repository state." Enter
tmux send-keys -t belltower-test "/help" Enter
tmux send-keys -t belltower-test "/connection openai" Enter
tmux send-keys -t belltower-test "/model o4-mini" Enter
tmux send-keys -t belltower-test "/use local qwen3.5:latest" Enter
tmux send-keys -t belltower-test "/defaults" Enter
tmux send-keys -t belltower-test "/defaults use openai o4-mini" Enter
tmux send-keys -t belltower-test "/inspect session" Enter
tmux send-keys -t belltower-test "/inspect tool call-123" Enter
tmux send-keys -t belltower-test "/new" Enter
tmux send-keys -t belltower-test "/branch" Enter
tmux send-keys -t belltower-test "/cancel" Enter
tmux send-keys -t belltower-test "/steer answer briefly" Enter
tmux send-keys -t belltower-test Up
tmux send-keys -t belltower-test Down
tmux send-keys -t belltower-test C-l
tmux send-keys -t belltower-test C-j
tmux send-keys -t belltower-test Escape
tmux send-keys -t belltower-test C-c
tmux send-keys -t belltower-test C-b
tmux send-keys -t belltower-test '['
tmux send-keys -t belltower-test ']'
```

Use `capture-pane` after each interaction to inspect what the user would see.

Slash-command scope matters:

- `/connection`, `/model`, and `/use` change only the current session.
- `/defaults ...` persists future-session defaults in `~/.config/belltower/config.toml`.

Composer-specific checks:

- The default empty composer is two visible rows tall.
- `Ctrl-J` inserts a literal newline instead of submitting.
- `Shift+Enter` and `Alt+Enter` should also insert a newline when the terminal reports modified-enter keys correctly.
- When multiline input or a transient slash panel expands the shell, the new rows should appear below the existing composer first; they should not open visual space above the live transcript unless the shell has reached the terminal bottom.
- Once the composer has draft text, the idle footer should collapse rather than leaving an extra reserved row under the draft.
- `Ctrl-A`, `Ctrl-E`, `Ctrl-K`, `Ctrl-U`, and `Ctrl-W` should behave like shell editing, not like transcript navigation.
- history browse should return the composer to the current draft when the user comes back down past the newest committed input.
- bracketed paste bursts should preserve embedded newlines in the composer instead of triggering incremental submits.

## 4. Minimum TUI Test Matrix

For any meaningful TUI change, exercise:

1. startup
   - first-run path
   - auto-start server path
   - resume existing session path

2. chat loop
   - send prompt
   - receive streamed response
   - queue a second prompt while the first is still running
   - verify queued follow-up preview appears in the shell-top status stack above the composer
   - cancel a running turn
   - steer a running or approval-paused turn
   - verify the active turn stays visually contiguous near the composer
   - verify committed history appends into scrollback instead of mutating in place
   - verify committed history insertion does not redraw over older terminal scrollback
   - use terminal scrollback for older transcript history
   - verify drag selection and copy work on transcript text

3. tool visibility
   - visible tool activity
   - visible approval state
   - visible denial or failure state
   - persistent `call_id` values on compact tool transcript lines
   - `/inspect tool <call-id>` flushes large output fully into scrollback without clipping

4. navigation
   - session switch if available
   - branch switch
   - branch creation
   - slash-command session/provider/model changes
   - slash inspection of session and tool calls with `/inspect`
    - inspect output for session and tool-call drill-down
   - slash layout raise/cancel/collapse behavior
   - only one transient bottom surface is visible at a time: footer, slash menu, approval chooser, or question prompt

5. degraded states
   - missing auth
   - unhealthy provider
   - unreachable server
   - event-stream reconnect path

## 5. Scripted Acceptance Gates

The foundation acceptance runner includes deterministic TUI gates by default:

```bash
make acceptance
```

Those gates run `scripts/tui_acceptance.sh deterministic`, which covers the
Codex-style committed/live transcript seam, command surfaces, canonical
approval/question panels, operator fixtures, and inspection/export consumers.
They are CI-safe and do not require a real terminal multiplexer.

For the full-screen scripted smoke, run:

```bash
BELLTOWER_TUI_ACCEPTANCE_TMUX=1 make acceptance
```

or directly:

```bash
bash scripts/tui_acceptance.sh tmux
```

The tmux gate starts a temporary `bt-server`, a local OpenAI-compatible mock
provider, and `bt-tui` in a fixed-size tmux pane. It then drives:

- `/doctor`
- `/use local bt-tui-mock`
- a configurable long-session prompt loop
- an approval-producing mock turn
- `/inspect session`
- `/compact`
- `/export jsonl exports/acceptance.jsonl`

By default the scripted tmux gate sends 12 long-session turns so local developer
acceptance stays fast. To run the M4.1 stress variant:

```bash
BELLTOWER_TUI_ACCEPTANCE_TMUX=1 BELLTOWER_TUI_ACCEPTANCE_TURNS=500 make acceptance
```

Scope:

- validates that the TUI can operate as the primary interface against a real
  server process and a streaming provider
- validates that slash/operator commands are durable operator-visible history
  rather than local-only UI output
- validates that approval, inspection, compaction, and export remain reachable
  through the full-screen interface

Not covered:

- live OpenAI/Anthropic/local-model quality
- every terminal emulator's modified-key encoding
- one-week dogfood friction evidence

## 6. Human Milestone Recipes

## Local model happy path

```bash
tmux new-session -d -s belltower-local -x 100 -y 30
tmux send-keys -t belltower-local "cd /Users/sinabooeshaghi/projects/frollo/belltower && cargo run -p belltower --" Enter
sleep 3
tmux capture-pane -t belltower-local -p
tmux send-keys -t belltower-local "Write one sentence about Belltower." Enter
sleep 3
tmux capture-pane -t belltower-local -p
tmux kill-session -t belltower-local
```

## Approval flow

Use a prompt that should trigger `write` or `shell`, then:

```bash
tmux send-keys -t belltower-test Down
tmux send-keys -t belltower-test Enter
tmux capture-pane -t belltower-test -p
tmux send-keys -t belltower-test Down
tmux send-keys -t belltower-test Down
tmux send-keys -t belltower-test Down
tmux send-keys -t belltower-test Enter
tmux capture-pane -t belltower-test -p
```

Expected:

- approval state is obvious
- the approval chooser appears in the transient panel below the composer
- the approval chooser remains selectable if a draft prompt is present or the approval-requesting turn is still locally in flight
- Up and Down move between `Approve once`, `Approve for session`, `Approve always`, and `Deny`
- `Enter` confirms the highlighted choice
- the decision is reflected in the transcript
- the session continues or fails truthfully

## Queueing flow

Use a slow prompt, then send a second prompt before the first turn finishes:

```bash
tmux send-keys -t belltower-test "Summarize the repository in detail." Enter
tmux send-keys -t belltower-test "Now give me only the top three risks." Enter
tmux capture-pane -t belltower-test -p
```

Expected:

- the second prompt is queued instead of rejected
- the queued state is visible truthfully through the current interaction state or `/queue` inspection
- once the first turn finishes, the queued prompt is sent automatically

## Slash layout flow

Exercise the Codex-style slash layout:

```bash
tmux send-keys -t belltower-test "/"
tmux capture-pane -t belltower-test -p
tmux send-keys -t belltower-test Backspace
tmux capture-pane -t belltower-test -p
tmux send-keys -t belltower-test "hello" Enter
tmux capture-pane -t belltower-test -p
```

Expected:

- typing `/` opens the command panel below the composer
- the visible chat region shifts upward only by the amount of newly reserved panel space
- deleting `/` hides the command panel and returns the layout to the compact default state immediately
- executing a slash command also returns the layout to the compact default state once the panel closes

## Interrupt and steer flow

Use a slower prompt, then interrupt it:

```bash
tmux send-keys -t belltower-test "Explain this repository in detail." Enter
tmux send-keys -t belltower-test C-c
tmux capture-pane -t belltower-test -p
tmux send-keys -t belltower-test "/steer answer in one sentence" Enter
tmux capture-pane -t belltower-test -p
```

Expected:

- `Ctrl-C` requests cancellation of the in-flight turn instead of quitting the TUI
- queued follow-up messages do not auto-dispatch after the cancelled turn completes
- `/steer ...` is accepted while a turn is active or paused on approval
- the next model turn reflects the steering message instead of ignoring it

## Session model switching

Use slash commands inside the TUI:

```bash
tmux send-keys -t belltower-test "/connection openai" Enter
tmux capture-pane -t belltower-test -p
tmux send-keys -t belltower-test "/model o4-mini" Enter
tmux capture-pane -t belltower-test -p
```

Expected:

- subsequent status and inspection surfaces reflect the new connection and model truthfully
- subsequent prompts run on the selected session-local provider/model

## Tool inspection

Use slash commands inside the TUI:

```bash
tmux send-keys -t belltower-test "/inspect session" Enter
tmux capture-pane -t belltower-test -p
tmux send-keys -t belltower-test "/inspect tool call-123" Enter
tmux capture-pane -t belltower-test -p
```

Expected:

- chat transcript is append-only and immutable once printed
- tool calls and tool results include persistent `call_id` values that can be inspected later
- `/inspect session` renders a structured session summary in the transcript
- `/inspect tool <call-id>` renders a detailed view of the selected tool call without rerendering prior transcript output
- if the inspect payload is too large for the live region, it flushes fully into scrollback in-order rather than clipping in place

## Reconnect behavior

When possible during development, briefly interrupt `bt-server` or the SSE stream and observe:

- a visible reconnecting state appears near the composer/live tail
- input remains usable unless a send or approval action is already running; pending approvals remain selectable over drafts and in-flight send state
- once the stream recovers, missed events append without duplicate transcript entries

## 6. What To Capture In Bug Reports

When a TUI test fails, capture:

- terminal size
- command used to start the TUI
- active connection and model
- last visible screen output from `tmux capture-pane`
- expected behavior
- actual behavior

If the failure involves approvals, branches, or MCP state, include those states explicitly.
