# TUI Functional Requirements

This document captures the behavior that tests must protect for the inline Belltower TUI.

## Core Interaction

- The inline shell is the only supported TUI layout.
- Transcript history is append-only and new entries render immediately above the composer.
- The live current-turn region behaves like the newest part of the chat, not like separate chrome.
- Waiting state text may appear there temporarily until tool or assistant output becomes visible.
- The composer is multiline, unboxed, and visually aligned with compact user transcript rendering.
- The default idle layout keeps a quiet inline footer below the composer instead of a separate boxed status pane.

## Slash Commands

- Typing `/` opens the command panel below the composer.
- Backspacing away the slash hides the menu and returns the layout to the compact default state.
- Executing a slash command also returns the layout to the compact default state once the panel closes.
- `Tab` applies slash completion.
- `Up` and `Down` navigate command choices while the slash menu is open.

## Approval and Ask Flows

- A pending approval replaces the slash panel with an approval chooser.
- The approval chooser remains visible over an unsent composer draft; resolving the approval must not clear that draft.
- The approval chooser remains selectable while the turn that requested approval is still represented as in flight locally.
- A pending ask replaces the slash panel with compact question guidance.
- `Up`, `Down`, and `Enter` operate on approval choices when the approval chooser is active.
- Approval and ask flows do not regress queued-input behavior.

## Keyboard Editing

- `Shift+Enter`, `Alt+Enter`, and `Ctrl-J` insert newlines.
- `Option+Left/Right` move by word.
- `Option+Backspace` deletes the word to the left.
- The Option editing path accepts terminal Alt/Meta/Super/Hyper encodings and escape-prefixed Backspace/DEL variants.
- `Ctrl-A`, `Ctrl-E`, `Ctrl-K`, `Ctrl-U`, and `Ctrl-W` match shell-style editing.
- `Up` and `Down` browse sent user-input history outside menu modes.

## Layout and Rendering

- User transcript entries are compact and unboxed.
- Operator output is compact and unboxed.
- User transcript entries render with a stable `›` prefix and aligned continuation lines.
- Tool activity renders as compact action rows with a `•` prefix.
- Tool results render as subordinate rows with `└` under the relevant tool activity.
- The current turn keeps the optimistic user prompt, streamed tool activity, streamed assistant text, and temporary waiting states contiguous above the composer.
- Canonical committed transcript entries move into terminal scrollback immediately instead of lingering in separate footer or viewport chrome.
- The active turn owns an ordered mutable segment list for assistant text and tool blocks while the turn is live.
- Once part of an active-turn segment has been emitted to scrollback, that exact portion is never emitted again.
- Canonical `message.appended` events reconcile or finalize active-turn segments; they do not rebuild already-emitted tool or assistant output from the full message payload.
- Assistant text that occurs after a tool block becomes a fresh assistant segment rather than being merged back into the tool block.
- Multiline composer growth consumes space above the composer by moving the live current-turn region upward.
- Slash, approval, and question panels consume space below the composer and disappear immediately when canceled or resolved.
- The idle layout keeps a compact inline footer; it does not switch to a separate boxed status pane or leave blank reserved chrome.
- Explicit `Thinking` status only appears before meaningful tool or assistant output is visible.
- Consecutive tool rows stay visually contiguous without paragraph gaps between the tool action and its result.

## Startup and Terminal Environment

- The visible pane starts clean.
- Operator guidance comes from the placeholder, `/help`, slash completion, and transient chat-visible activity rather than startup filler.

## Minimum Test Matrix

- empty composer
- typing/working state in the live current-turn region
- quiet inline footer in the idle layout
- slash popup appearance, filtering, selection, and completion
- slash delete returning to the compact default layout
- slash execution returning to the compact default layout
- approval panel replacing slash panel
- question panel replacing slash panel
- compact unboxed transcript rendering
- append ordering directly above the composer
- live tool previews from streamed tool-call deltas
- multiline composer growth and shrink
