# TUI Reference Review — 2026-04-02

This note records a direct tmux-based review of the installed reference harnesses:

- `codex`
- `claude`
- `hermes`
- `pi`

The goal is not to copy any one harness wholesale.
It is to identify the interaction patterns that make a terminal harness feel trustworthy, efficient, and pleasant.

## How This Review Was Done

Each harness was launched in its own tmux session and observed in startup/help states.

Commands used:

```bash
tmux new-session -d -s ref-codex 'cd /Users/sinabooeshaghi/projects/frollo && codex'
tmux new-session -d -s ref-claude 'cd /Users/sinabooeshaghi/projects/frollo && claude'
tmux new-session -d -s ref-hermes 'cd /Users/sinabooeshaghi/projects/frollo && hermes'
tmux new-session -d -s ref-pi 'cd /Users/sinabooeshaghi/projects/frollo && PI_CODING_AGENT_DIR=/Users/sinabooeshaghi/projects/frollo/belltower/.tmp-pi PI_OFFLINE=1 pi'
tmux capture-pane -p -t <session> -S -120
```

This is intentionally a product-interaction review, not just a source-code inspection.

## What The Reference Harnesses Do Well

### Codex

What stands out:

- very compact startup surface
- one small identity card with only the highest-signal context
- composer-first layout with minimal persistent chrome
- warnings and startup problems are printed into transcript/scrollback instead of being hidden in a side panel

What is worth copying:

- strong bias toward a shell-first interaction model
- minimal persistent footer noise
- small startup card, then immediate focus on the composer

What is not worth copying directly:

- upgrade prompts that block the first interaction
- extremely terse discoverability for less-expert users unless Belltower has a better first-run story elsewhere

### Claude Code

What stands out:

- polished landing screen
- very strong shortcut discoverability
- composer and help affordances are visually clear
- the UI feels intentional and premium

What is worth copying:

- high-quality shortcut discoverability
- strong prompt/composer affordance
- operator confidence through clear keyboard help

What is not worth copying directly:

- too much persistent framing for Belltower's current shell-first direction
- a large welcome card is better as a first-run or optional surface than as the permanent norm

### Hermes

What stands out:

- excellent discoverability around capabilities, slash commands, tools, and skills
- fixed input area at the bottom with clear placeholder/hint behavior
- strong operator cues for multiline input and current model/session context

What is worth copying:

- thoughtful multiline input behavior
- clear prompt-area hints
- bottom-fixed input with strong affordance
- queue/interrupt model as an explicit operator concept

What is not worth copying directly:

- startup density is too high
- the first screen is overloaded with tools, skills, and branding
- it teaches everything at once instead of preserving focus

### pi

What stands out:

- the strongest shell-native feel of the four
- startup help is printed once into scrollback, not pinned permanently in footer chrome
- very lean status line
- the input feels like a real terminal editor, not a boxed chat widget

What is worth copying:

- startup affordances as scrollback, not persistent clutter
- small stable status line
- strong keyboard ergonomics
- warning when tmux extended keys are off

What is not worth copying directly:

- startup output can feel too raw and technical
- lack of a clearer structured session/provider/status summary than Belltower probably needs

## What This Means For Belltower

Belltower's direction is still correct:

- single inline/main-screen TUI
- terminal-native scrollback
- terminal-native drag selection
- fixed composer at the bottom
- immutable chat history
- detailed drill-down through `inspect` instead of transcript rerendering

But the comparison shows that Belltower still has too much persistent chrome for the amount of information it provides.

The biggest remaining TUI product issue is not correctness now.
It is interaction quality and visual discipline.

## High-Value TUI Improvements

### 1. Reduce the persistent footer command clutter

Today Belltower keeps a long slash-command footer visible all the time.

The references suggest a better split:

- keep one or two contextual hints near the composer
- move the rest of command discoverability into:
  - startup scrollback
  - `/help`
  - slash autocomplete

Recommendation:

- replace the long persistent command list with a much smaller hint line such as:
  - `/help for commands`
  - `/inspect tool <id> for tool details`
- let `/help` be the real command surface

This would make the bottom region calmer immediately.

### 2. Simplify the composer chrome

The current boxed composer is better than the earlier versions, but it is still heavier than the best references.

The references suggest:

- the composer should feel like the main control surface
- but it should not visually dominate the transcript

Recommendation:

- keep the ghost placeholder
- reduce the visual weight of the composer border
- consider a one-line prompt glyph plus growing multiline field instead of a full heavy box

This should be done carefully, because the boxed version currently helps with alignment and discoverability.

### 3. Treat startup guidance as transcript, not footer

pi does this particularly well.
Claude does it with a welcome card.
Belltower should choose the shell-first version.

Recommendation:

- print a short startup help block into scrollback once:
  - multiline input
  - `/help`
  - `/inspect tool <id>`
  - `Esc` behavior
- do not keep that information permanently pinned in the footer

This would improve first-run discoverability without increasing steady-state clutter.

### 4. Keep the active turn visually contiguous

This is already much better than before, but it is still the most sensitive part of the TUI.

Recommendation:

- keep the newest user message, working indicator, streamed assistant text, and immediate tool activity in one contiguous live region above the composer
- avoid any dynamic viewport behavior that can push the active turn upward unexpectedly
- preserve the current rule that large inspect output flushes fully into scrollback rather than clipping in the live region

This remains the main correctness-sensitive interaction path.

### 5. Add explicit tmux key-capability detection

pi's tmux warning is useful.

Belltower depends on modified keys for:

- `Shift+Enter`
- `Alt+Left`
- `Alt+Right`

Recommendation:

- detect when extended keys are unavailable in tmux
- show a one-time startup notice explaining that some modified keys may not be reported correctly

That will reduce confusion when multiline editing appears flaky in tmux.

### 6. Continue modularizing `bt-tui` around interaction seams

Codex and pi both reinforce this.

Current Belltower seams are improving, but `main.rs` is still too large.

Next high-value splits:

- slash-command parsing/dispatch out of `main.rs`
- input/editor behavior into its own module
- stream reconciliation and transcript-merge policy into its own module

This is not just code cleanliness.
It directly lowers the risk of new TUI regressions.

## Things Belltower Should Not Chase

### Do not copy Hermes's startup density

Belltower should not dump full tool and skill inventories at launch.
That makes the harness feel powerful, but not focused.

### Do not copy Claude's full welcome-screen weight as the default

Belltower should stay shell-first.
If it gets a richer welcome experience later, that should be a first-run or explicit help surface.

### Do not reintroduce mutable transcript rendering

The recent move away from replay-driven historical mutation was correct.
Inspectability is a better answer than transcript verbosity toggles.

## Recommended Next TUI Slice

If the goal is to keep making the TUI first-class, the next slice should be:

1. shrink the persistent footer command list dramatically
2. move startup/operator help into one-time transcript output
3. add tmux modified-key detection/warning
4. continue extracting command/input logic out of `bt-tui/src/main.rs`

That would improve both product feel and implementation stability.
