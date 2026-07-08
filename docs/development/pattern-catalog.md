# Belltower Development Pattern Catalog

This document catalogs development patterns from `pi-mono` and Hermes that are worth copying, adapting, or explicitly rejecting for Belltower.

It complements:

- [`../architecture/overview.md`](../architecture/overview.md)
- [`../planning/target-implementation-goals.md`](../planning/target-implementation-goals.md)
- [`../planning/subsystem-parity-matrix.md`](../planning/subsystem-parity-matrix.md)

Those documents define the product and subsystem targets.
This document defines the development patterns that should guide how we build the product.

The main source reviewed here is `pi-mono`'s [`AGENTS.md`](/tmp/belltower-case-studies/pi-mono/AGENTS.md), plus adjacent package/docs material from `pi-mono` and Hermes.

## 1. Why This Exists

`pi-mono` is mature enough that some of its repo conventions are not arbitrary. They encode lessons about:

- how to keep a multi-subsystem agent project coherent
- how to avoid regressions in a fast-moving harness
- how to keep operator-facing UX and runtime behavior aligned
- how to let multiple agents or humans work safely in one repo

Belltower should not copy these rules verbatim.
The stack, language, release model, and architecture are different.
But many of the patterns are reusable with minimal translation.

The right approach is:

- copy what is structurally valuable
- adapt what depends on TypeScript or `pi`-specific architecture
- reject what does not fit Rust, Belltower, or our actual workflow

## 2. Pattern Summary Table

| Pattern | Source | Recommendation | Why |
| --- | --- | --- | --- |
| Single entrypoint + coherent operator commands | Hermes, `pi` | Adopt | Belltower already wants `belltower`, `setup`, `login`, `model`, `doctor`, `status` to tell one story. |
| Package/system table for the repo | `pi` | Adopt | Helps keep a growing harness grounded and makes implementation planning concrete. |
| Per-subsystem implementation checklist | `pi` | Adopt | Good fit for provider/tool/runtime work in Belltower. |
| Strict git hygiene for parallel agents | `pi` `AGENTS.md` | Adopt | We are already operating in a dirty shared worktree and need exactly these protections. |
| Explicit TUI testing recipe with `tmux` | `pi` `AGENTS.md` | Adopt | Very useful for Belltower's TUI and full-screen regressions. |
| Coherent setup/login/model/doctor/status flows | Hermes | Adopt | One of the best practical patterns in Hermes. |
| Truthful provider and capability reporting | Hermes, `pi` | Adopt | Belltower needs this to avoid misleading auth/model/runtime states. |
| Provider integration checklist across repo surfaces | `pi` `AGENTS.md` | Adopt and rewrite | Extremely useful, but must be rewritten for Belltower crates and Rust tests. |
| Strong command discipline for local verification | `pi` `AGENTS.md` | Adapt | Good pattern, but the actual allowed commands differ in Rust. |
| Changelog discipline | `pi` `AGENTS.md` | Adapt later | Useful once Belltower starts cutting releases. |
| Subscription login assumptions | `pi`, Hermes | Adapt cautiously | Useful as a future auth architecture pattern, not current Belltower runtime behavior. |
| Node/type-specific code quality rules | `pi` `AGENTS.md` | Reject as written | They are TypeScript-specific, but the underlying idea should be translated to Rust. |
| GitHub issue/PR workflow specifics | `pi` `AGENTS.md` | Partially adopt | Useful for OSS process later, but not core to the current harness implementation effort. |
| OSS weekend automation | `pi` `AGENTS.md` | Reject | Not relevant to current Belltower development. |
| Whole-repo porting strategy | None | Reject | We want selective subsystem parity, not wholesale translation of a different architecture. |

## 3. Candidate Belltower Repo Rules

This section is the practical output.
It is the closest thing to a future Belltower `AGENTS.md`.

## 3.1 First-Read Rules

Pattern from `pi`:

- orient the agent to the repo structure before making changes
- read module-specific docs before modifying that module

Belltower adaptation:

- if the user has not given a narrow task, first read:
  - [`../architecture/overview.md`](../architecture/overview.md)
  - [`../planning/target-implementation-goals.md`](../planning/target-implementation-goals.md)
  - [`../planning/subsystem-parity-matrix.md`](../planning/subsystem-parity-matrix.md)
- then read the crate-local sources for the affected subsystem
- if the task spans multiple subsystems, read the relevant crate entrypoints in parallel

Why this is worth copying:

- it forces changes to stay consistent with the intended architecture
- it reduces random local fixes that cut across crate boundaries incorrectly

## 3.2 Code Quality Rules

Pattern from `pi`:

- explicit quality rules instead of vague "keep code clean" language

Belltower adaptation:

- no `unsafe` in crate code unless there is a compelling documented reason
- do not bypass `bt-protocol` or `bt-client` for convenience in TUI code
- do not bypass `bt-session` for telemetry or export logic
- do not hardcode provider support checks in random call sites; keep them centralized
- do not fake readiness from credential presence alone when real validation is possible
- do not silently remove intentional user-facing behavior without checking
- keep keybindings configurable in the TUI rather than hardcoding terminal key matches inline

This should become a real Belltower rule set.

## 3.3 Verification Discipline

Pattern from `pi`:

- after code changes, run the canonical repo checks
- if you change tests, run those tests until they pass

Belltower adaptation:

- after code changes, run targeted verification appropriate to the touched crates
- the default verification ladder is:
  - `cargo fmt --all`
  - `cargo check -p <affected-crate>` or `cargo check --workspace`
  - `cargo test -p <affected-crate>` and any adjacent affected crates
- if you add or modify a test, you must run that test-bearing crate until it passes
- for cross-cutting changes, run the smallest set of crates that exercises the changed path
- avoid long-running or interactive commands unless the task specifically requires them

Why this is worth copying:

- it keeps the harness shippable while still allowing incremental work
- it creates a reliable "slice" workflow for a multi-crate Rust workspace

## 3.4 Provider Integration Checklist

This is one of the most useful patterns in `pi`'s `AGENTS.md`.
It should be copied in Belltower-specific form.

When adding a new provider in Belltower, update at least:

1. `bt-core`
   - provider IDs and any shared auth/config/type support

2. `bt-providers`
   - provider implementation
   - validation path
   - stream parsing
   - tool-call translation
   - pricing/context-window support if applicable

3. `bt-core` embedded config/catalog data
   - provider catalog
   - pricing catalog
   - context-window catalog
   - default model/fallbacks

4. launcher and operator surfaces
   - `belltower` login/setup/model/doctor/status

5. protocol/runtime/server surfaces
   - any provider exposure or validation state surfaced through API

6. documentation
   - quickstart
   - provider setup docs
   - target support matrix

7. tests
   - provider fixture/conformance tests
   - validation path tests
   - stream parser tests
   - tool-call translation tests
   - launcher truthfulness tests if support state changes

This should become an explicit checklist in the repo.

## 3.5 TUI Testing Pattern

Pattern from `pi`:

- use `tmux` to test the interactive terminal UI at fixed dimensions

Belltower adaptation:

```bash
# create session with stable terminal size
tmux new-session -d -s belltower-test -x 100 -y 30

# start the launcher from source
tmux send-keys -t belltower-test "cd /Users/sinabooeshaghi/projects/frollo/belltower && cargo run -p belltower --" Enter

# capture output
sleep 3 && tmux capture-pane -t belltower-test -p

# send input
tmux send-keys -t belltower-test "hello" Enter

# send escape or control keys
tmux send-keys -t belltower-test Escape
tmux send-keys -t belltower-test C-l

# cleanup
tmux kill-session -t belltower-test
```

This should become an official Belltower testing recipe once the TUI slice is stronger.

Why it matters:

- full-screen TUIs regress in ways unit tests do not catch
- fixed-size terminal testing makes rendering bugs reproducible

## 3.6 Parallel-Agent Git Safety

This is already directly relevant to Belltower work and should be copied almost wholesale.

Rules worth adopting directly:

- only stage files changed in the current slice
- never use `git add -A` or `git add .`
- always inspect `git status` before committing
- never use destructive cleanup commands in a shared dirty worktree
- if a conflict appears in a file outside the current slice, stop and ask

This aligns with how we have already been working in the repo and should become explicit policy.

## 3.7 Changelog and Release Discipline

Pattern from `pi`:

- explicit release process
- immutable released changelog sections
- new entries only under `Unreleased`

Belltower adaptation:

- adopt later, once Belltower is cutting real versions
- keep the pattern, but do not overbuild release machinery before the stable harness exists

This is valuable, but it is not a current critical path item.

## 3.8 Auth Import and Migration Helpers

Pattern from Hermes and `pi`:

- import or reuse existing auth state when possible
- make setup/login/model flows reflect the same credential picture

Belltower adaptation:

- add import helpers into `bt-auth` for:
  - prior Belltower auth data
  - possibly external stores later when the auth semantics really match
- keep `doctor` and `status` authoritative for what is actually configured and usable

This is one of the best candidate slices to copy next.

## 3.9 Docs As Operational Surfaces

Pattern from `pi`:

- docs are not just prose; they encode workflows, command contracts, and extension points

Belltower adaptation:

- keep architecture docs, requirements docs, subsystem docs, and operator docs separate
- make quickstart and setup docs track the actual launcher behavior closely
- add subsystem-specific implementation checklists where the repo has recurring multi-file change patterns

This is worth copying directly.

## 4. Adopt / Adapt / Reject Table

| Pattern | Adopt | Adapt | Reject | Notes |
| --- | --- | --- | --- | --- |
| Single entrypoint operator UX | yes |  |  | Already core to Belltower. |
| Repo/module-first reading discipline | yes |  |  | Translate to Belltower docs and crates. |
| Explicit quality rules | yes |  |  | Rewrite for Rust and Belltower invariants. |
| Cargo-based verification ladder |  | yes |  | Same pattern, different commands. |
| TUI `tmux` testing recipe | yes |  |  | High value for Belltower. |
| Provider integration checklist |  | yes |  | Must be rewritten crate-by-crate. |
| Git safety rules for shared worktrees | yes |  |  | Directly applicable now. |
| Release/changelog discipline |  | yes |  | Useful later, not urgent now. |
| GitHub issue comment/label automation |  | yes |  | Useful for OSS process, not harness-critical. |
| OSS weekend mode |  |  | yes | Repo-specific, not relevant. |
| TypeScript-specific typing rules |  | yes |  | Translate underlying intent to Rust. |
| Whole repo architecture port |  |  | yes | We want subsystem parity, not architectural replacement. |

## 5. Belltower-Specific Rules We Should Add That `pi` Does Not Cover

These are Belltower-specific because of the harness's telemetry-first design.

### Rule: No hidden telemetry bypasses

- if a provider, tool, or export path bypasses the canonical session store, treat it as a bug

### Rule: No hidden local execution paths

- if the TUI bypasses the protocol or runtime for convenience, treat it as architectural debt to remove

### Rule: Raw stream durability is non-optional

- if a provider path cannot persist raw output, it is not feature-complete

### Rule: Truthful readiness beats optimistic readiness

- credentials present is not the same as a healthy provider
- backend reachable is not the same as the configured model being usable

These should eventually live in a Belltower repo policy document or `AGENTS.md`.

## 6. Concrete Outputs We Should Produce From This Catalog

This catalog should turn into repo artifacts, not just a note.

Recommended outputs:

1. a Belltower [`AGENTS.md`](../../AGENTS.md)
   - much shorter than `pi`'s
   - Rust-specific
   - focused on architecture invariants, verification discipline, and git safety

2. a provider integration checklist doc
   - [`provider-integration-checklist.md`](./provider-integration-checklist.md)
   - a Belltower-specific version of the best part of `pi`'s provider checklist

3. a TUI testing recipe doc
   - [`tui-testing.md`](./tui-testing.md)
   - includes `tmux` workflow, fixed sizes, and capture instructions

4. an auth migration/import checklist
   - [`auth-migration-and-imports.md`](./auth-migration-and-imports.md)
   - tracks what existing stores can and cannot be imported honestly

## 7. Recommended Next Use Of This Catalog

The highest-value immediate uses are:

1. write a real Belltower [`AGENTS.md`](../../AGENTS.md)
   - keep it short, strict, and Rust-native

2. add a provider-integration checklist
   - [`provider-integration-checklist.md`](./provider-integration-checklist.md)
   - this will pay off quickly as provider support grows

3. add a TUI testing guide
   - [`tui-testing.md`](./tui-testing.md)
   - this will pay off immediately when we improve the interactive client

4. keep using the subsystem parity matrix for product scope
   - use this pattern catalog for development process

That split is important:

- [`../planning/subsystem-parity-matrix.md`](../planning/subsystem-parity-matrix.md) tells us what to build
- [`./pattern-catalog.md`](./pattern-catalog.md) tells us how to build it
