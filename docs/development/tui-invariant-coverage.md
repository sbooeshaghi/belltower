# TUI Invariant Coverage

This note records the current `bt-tui` coverage against the Codex cutover invariants in
[`codex-tui-cutover-note-20260407.md`](./codex-tui-cutover-note-20260407.md).

The goal is not to maximize test count. The goal is to keep each cutover invariant tied to a
concrete test so later TUI work does not silently reintroduce the old unstable model.

## Cutover invariants

### Committed history is append-only

- Covered by
  - `streamed_assistant_markdown_collapses_repeated_blank_lines`
  - `canonical_assistant_message_does_not_reprint_already_streamed_text`
  - `canonical_tool_call_message_reconciles_live_tool_without_duplicate_history`

These tests prove that once history is committed it is not reconstructed or reprinted by later
canonical events.

### Shift+Enter cannot erase or overwrite prior scrollback

- Covered by
  - `shift_enter_preserves_committed_scrollback_history`
  - `scrollback_insertion_with_space_below_viewport_uses_reverse_index_shift`

The first test keeps committed history stable while the composer grows. The second checks the
terminal-level reverse-index path that creates room for new bottom-shell height without destroying
history rows.

### Streamed assistant text appears incrementally in order

- Covered by
  - `streamed_assistant_text_commits_completed_lines_and_keeps_partial_tail_live`
  - `streamed_assistant_markdown_collapses_repeated_blank_lines`

These pin the newline-gated collector behavior: completed lines commit in order, incomplete tails
stay live.

### Tool calls and results appear where they happen and do not jump later

- Covered by
  - `tool_call_is_rendered_live_then_committed_immediately_on_result`
  - `tool_block_commits_before_later_summary_text_streams`
  - `canonical_assistant_summary_waits_for_tool_and_only_emits_unseen_suffix`
  - `streamed_assistant_lines_do_not_commit_ahead_of_live_tool_cells`
  - `completed_leading_tool_commits_while_later_tool_preview_stays_live`

These tests cover the mutable live tool cell, immediate commit at real boundaries, and assistant
summary ordering after tool completion.

### The viewport stays anchored above the composer and footer

- Covered by
  - `chat_layout_stacks_live_transcript_above_bottom_shell`
  - `chat_layout_keeps_aux_rows_above_composer_and_footer`

These pin the inline layout contract: live transcript, shell auxiliary rows, composer, and footer
stay in the intended stack order.

### Multiline compose grows inside the bottom shell before it grows the viewport

- Covered by
  - `composer_body_height_tracks_input_without_extra_headroom`
  - `inline_viewport_height_grows_when_active_cell_is_present`
  - `footer_hides_when_composer_has_draft_text`

These ensure the composer grows as a shell-owned surface first, instead of flattening the footer
and transcript into one content-height block.

### Slash-panel transitions collapse cleanly

- Covered by
  - `slash_surface_collapses_when_slash_is_deleted`
  - `slash_surface_takes_precedence_over_pending_question_prompt`

These keep the transient command surface coherent with the compact default shell.

### The final end-of-turn state matches the streamed state

- Covered by
  - `pure_conversational_turn_does_not_emit_final_work_separator`
  - `tool_only_turn_emits_final_work_separator_on_completion`
  - `canonical_ask_result_commits_completed_history_cell`

These tests pin end-of-turn reconciliation so the user does not see one streamed story and a
different final transcript.

## Operator-surface fixtures

The operator-readable surfaces are pinned by committed fixtures under
`crates/bt-tui/tests/fixtures/operator/`.

- `/help`
  - `operator_surface_help_matches_fixture`
- `/status`
  - `operator_surface_status_matches_fixture`
- `/doctor`
  - `operator_surface_doctor_matches_fixture_and_fits_80_columns`
- `/models`
  - `operator_surface_models_matches_fixture`
- `/inspect session`
  - `operator_surface_session_inspect_matches_fixture`
- `/inspect execution`
  - `operator_surface_execution_inspect_matches_fixture`
- `/inspect tool <call-id>`
  - `operator_surface_tool_call_inspection_matches_fixture`
- approval chooser
  - `operator_surface_approval_prompt_matches_fixture`
- canonical error projection
  - `operator_surface_error_projection_matches_fixture`

Intentional UX changes to these surfaces require explicit fixture updates. Accidental drift should
fail tests.
