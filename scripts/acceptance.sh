#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

run_exact() {
  local package="$1"
  local test_name="$2"
  shift 2
  local listed_tests
  listed_tests="$(cargo test -p "$package" "$test_name" -- --list)"
  if ! grep -Fqx "${test_name}: test" <<<"$listed_tests"; then
    printf '\nERROR: exact test not found: %s %s\n' "$package" "$test_name" >&2
    exit 1
  fi
  run cargo test -p "$package" "$test_name" -- --exact "$@"
}

skip_live() {
  local milestone="$1"
  local reason="$2"
  printf '\n==> SKIP %s: %s\n' "$milestone" "$reason"
}

printf 'Belltower foundation acceptance\n'
printf 'Scope: deterministic milestone proxies that do not require live provider credentials.\n'
printf 'Not covered by default: live provider turns unless BELLTOWER_LIVE_ACCEPTANCE=1 is set.\n'

# Scope: install -> health/capability/protected-route coherence.
# Not covered: interactive setup prompts or actual local server process launch.
run_exact bt-server tests::health_is_public_and_protected_routes_require_auth
run_exact bt-server tests::server_info_reports_versions_and_capabilities
run cargo test -p belltower
run cargo test -p bt-readiness
run cargo test -p bt-tui operator_surface_doctor_matches_fixture_and_fits_80_columns
run cargo test -p bt-tui operator_surface_status_matches_fixture

# Scope: local and cloud provider happy paths when explicitly enabled.
# Not covered by default: live first-token latency, network retries, real model fallback.
if [[ "${BELLTOWER_LIVE_ACCEPTANCE:-0}" == "1" ]]; then
  run bash scripts/live_acceptance.sh all
else
  skip_live "M1 local model happy path" "set BELLTOWER_LIVE_ACCEPTANCE=1 to run against a configured local backend"
  skip_live "M2 cloud provider happy path" "set BELLTOWER_LIVE_ACCEPTANCE=1 to run against real provider credentials"
fi
run cargo test -p bt-core --test pricing_catalog
run cargo test -p bt-readiness inspect_connection_models_filters_openai_inventory_to_curated_models
run cargo test -p bt-readiness inspect_connection_models_filters_anthropic_inventory_to_curated_models

# Scope: protocol health, typed errors, replay, and durable cancel/steer inspection.
# Not covered: long-lived client reconnects over a real network boundary.
run_exact bt-server tests::error_envelope_matrix_pins_client_visible_error_classes
run_exact bt-server tests::event_stream_replays_across_process_restart_without_duplicates
run_exact bt-server tests::control_persistence_survives_restart_via_protocol_path

# Scope: built-in tool execution, approval pause/resume, operator-command durability.
# Not covered: arbitrary third-party tool policies.
run_exact bt-server tests::approval_round_trip_executes_tool_and_continues_turn
run_exact bt-server tests::operator_shell_commands_record_tool_and_command_events
run_exact bt-server tests::inspection_contract_min_reconstructs_canonical_session_truth

# Scope: session replay, raw chunks, and ordered SSE replay from persisted state.
# Not covered: provider-specific raw payload variants beyond current fixtures.
run_exact bt-server tests::session_round_trip_persists_messages_events_and_raw_chunks
run_exact bt-server tests::event_stream_replays_from_last_event_id_and_delivers_live_updates
run_exact bt-server tests::event_stream_recovers_from_broadcast_lag_by_replaying_store_events

# Scope: branch continuity and child-session lineage surfaces.
# Not covered: large branch trees or collaborative multi-user editing.
run_exact bt-server tests::branch_create_and_activate_preserve_handoff_context
run_exact bt-server tests::spawn_route_creates_child_session_with_lineage_and_events
run_exact bt-server tests::workflow_route_reports_parent_child_runtime_state

# Scope: runtime-owned compaction route smoke.
# Not covered: semantic quality of compressed context in a live model turn.
run_exact bt-server tests::compact_route_runs_runtime_owned_compaction

# Scope: MCP inventory/reload/degraded behavior and MCP-as-tool inspection parity.
# Not covered: arbitrary external MCP servers or transport-specific stress tests.
run_exact bt-server tests::mcp_routes_report_configured_servers
run_exact bt-server tests::degraded_mcp_servers_do_not_break_turn_execution

# Scope: durable instruction provenance and deterministic protocol fixtures.
# Not covered: skill authoring or self-improvement flows.
run_exact bt-session store::tests::event_log_reconstructs_turn_instruction_provenance
run cargo test -p bt-protocol --test stabilization_fixtures

# Scope: TUI committed/live rendering invariants, operator fixtures, and canonical inspection/export consumers.
# Not covered: full-screen tmux execution unless BELLTOWER_TUI_ACCEPTANCE_TMUX=1 is set.
run bash scripts/tui_acceptance.sh deterministic
if [[ "${BELLTOWER_TUI_ACCEPTANCE_TMUX:-0}" == "1" ]]; then
  run bash scripts/tui_acceptance.sh tmux
else
  skip_live "M4.1 TUI tmux smoke" "set BELLTOWER_TUI_ACCEPTANCE_TMUX=1 to run the full-screen scripted tmux gate"
fi

# Scope: export fidelity for bundle, JSONL, ShareGPT, HTML, and OTLP bundle shape.
# Not covered: live Phoenix/Arize ingestion; that remains the ignored live test.
run_exact bt-server tests::session_export_returns_legacy_bundle_jsonl_html_sharegpt_and_otlp

# Scope: portable session.bt artifact export, offline validation, import, diff, continuation,
# artifact refs, raw-chunk integrity, event-boundary branch forks, and filesystem CAS sync.
# Not covered: public registry transport or ORCID-signed publication workflows.
run_exact bt-session store::tests::portable_session_bundle_exports_and_validates_offline
run_exact bt-session store::tests::portable_session_bundle_import_round_trips_canonical_evidence
run_exact bt-session store::tests::portable_session_bundle_exports_explicit_artifact_refs
run_exact bt-session store::tests::portable_session_bundle_validation_rejects_tampered_events
run_exact bt-session store::tests::portable_session_bundle_validation_rejects_missing_raw_chunk_content
run_exact bt-session store::tests::portable_session_bundle_diff_reports_same_lineage_update
run_exact bt-session store::tests::portable_session_bundle_diff_reports_fork_divergence
run_exact bt-session store::tests::portable_session_bundle_continue_from_mid_node_bounds_context
run_exact bt-session store::tests::portable_session_bundle_exports_branch_fork_boundary
run_exact bt-session sync::tests::session_bundle_push_and_pull_round_trip_remote_head
run_exact bt-session sync::tests::session_bundle_push_rejects_stale_expected_head

printf '\nAcceptance runner completed. Live provider gates remain explicit skips unless configured.\n'
