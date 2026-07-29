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

run_deterministic() {
  printf 'Belltower TUI deterministic acceptance\n'
  printf 'Scope: Codex-style committed/live transcript invariants, command surfaces, control panels, and operator fixtures.\n'
  printf 'Not covered: real terminal emulator quirks, local port binding, or sustained 500-turn full-screen interaction.\n'

  run cargo test -p bt-tui
  run_exact bt-server tests::inspection_contract_min_reconstructs_canonical_session_truth
  run_exact bt-server tests::session_export_returns_legacy_bundle_jsonl_html_sharegpt_and_otlp
}

free_port() {
  python3 - "$@" <<'PY'
import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
}

wait_for_url() {
  local url="$1"
  local label="$2"
  for _ in $(seq 1 240); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  printf 'Timed out waiting for %s at %s\n' "$label" "$url" >&2
  return 1
}

capture_tmux() {
  local session="$1"
  tmux capture-pane -t "$session" -p -S -
}

wait_for_pane_text() {
  local session="$1"
  local needle="$2"
  for _ in $(seq 1 120); do
    if capture_tmux "$session" | grep -Fq "$needle"; then
      return 0
    fi
    sleep 0.25
  done
  printf 'Timed out waiting for pane text: %s\n' "$needle" >&2
  capture_tmux "$session" >&2 || true
  return 1
}

wait_for_pane_text_any() {
  local session="$1"
  shift
  local pane
  local needle
  for _ in $(seq 1 120); do
    pane="$(capture_tmux "$session")"
    for needle in "$@"; do
      if grep -Fq "$needle" <<<"$pane"; then
        return 0
      fi
    done
    sleep 0.25
  done
  printf 'Timed out waiting for pane text matching any expected value:\n' >&2
  printf '  %s\n' "$@" >&2
  capture_tmux "$session" >&2 || true
  return 1
}

send_line() {
  local session="$1"
  local text="$2"
  tmux send-keys -t "$session" -l "$text"
  tmux send-keys -t "$session" Enter
}

run_tmux_smoke() {
  if ! command -v tmux >/dev/null 2>&1; then
    printf 'ERROR: TUI tmux acceptance was requested but tmux is not installed.\n' >&2
    return 1
  fi

  local turns="${BELLTOWER_TUI_ACCEPTANCE_TURNS:-12}"
  local temp_dir
  temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/belltower-tui-acceptance.XXXXXX")"
  local config_dir="$temp_dir/config"
  local mock_port_file="$temp_dir/mock-provider.port"
  local mock_log="$temp_dir/mock-provider.log"
  local server_log="$temp_dir/bt-server.log"
  local tmux_session="bt-tui-acceptance-$$"
  local mock_pid=""
  local server_pid=""

  # Mirror teardown state into script-scoped variables so the EXIT trap handles
  # early returns and normal completion through the same exact-resource path.
  tui_acceptance_temp_dir="$temp_dir"
  tui_acceptance_tmux_session="$tmux_session"
  tui_acceptance_mock_pid=""
  tui_acceptance_server_pid=""

  cleanup() {
    if [[ -n "${tui_acceptance_tmux_session:-}" ]]; then
      tmux kill-session -t "$tui_acceptance_tmux_session" >/dev/null 2>&1 || true
    fi
    if [[ -n "${tui_acceptance_server_pid:-}" ]]; then
      kill "$tui_acceptance_server_pid" >/dev/null 2>&1 || true
      wait "$tui_acceptance_server_pid" 2>/dev/null || true
    fi
    if [[ -n "${tui_acceptance_mock_pid:-}" ]]; then
      kill "$tui_acceptance_mock_pid" >/dev/null 2>&1 || true
      wait "$tui_acceptance_mock_pid" 2>/dev/null || true
    fi
    if [[ -n "${tui_acceptance_temp_dir:-}" ]]; then
      rm -rf "$tui_acceptance_temp_dir"
    fi
  }
  trap cleanup EXIT

  mkdir -p "$config_dir"
  python3 scripts/tui_mock_provider.py --port-file "$mock_port_file" >"$mock_log" 2>&1 &
  mock_pid="$!"
  tui_acceptance_mock_pid="$mock_pid"
  for _ in $(seq 1 80); do
    [[ -s "$mock_port_file" ]] && break
    sleep 0.1
  done
  if [[ ! -s "$mock_port_file" ]]; then
    printf 'Mock provider did not publish a port.\n' >&2
    cat "$mock_log" >&2 || true
    return 1
  fi
  local mock_port
  mock_port="$(cat "$mock_port_file")"
  wait_for_url "http://127.0.0.1:${mock_port}/v1/models" "mock provider"

  local server_port
  server_port="$(free_port)"
  cat >"$config_dir/config.toml" <<EOF
[defaults]
default_connection = "local"

[context]
summary_connection = "local"

[connections.local]
provider = "openai-compatible"
base_url = "http://127.0.0.1:${mock_port}/v1"
default_model = "bt-tui-mock"
auth_sources = []
model_fallbacks = ["bt-tui-mock"]
discoverable_model_selectors = [{ kind = "exact", value = "bt-tui-mock" }]
EOF

  BELLTOWER_CONFIG_DIR="$config_dir" \
    cargo run -p bt-server -- \
      --host 127.0.0.1 \
      --port "$server_port" \
      --database "$temp_dir/belltower.sqlite" >"$server_log" 2>&1 &
  server_pid="$!"
  tui_acceptance_server_pid="$server_pid"
  if ! wait_for_url "http://127.0.0.1:${server_port}/health" "bt-server"; then
    printf '\n==> bt-server log\n' >&2
    cat "$server_log" >&2 || true
    return 1
  fi

  tmux new-session -d -s "$tmux_session" -x 100 -y 30
  tmux send-keys -t "$tmux_session" \
    "cd \"$root\" && BELLTOWER_CONFIG_DIR=\"$config_dir\" cargo run -p bt-tui -- --server \"http://127.0.0.1:${server_port}/\" chat --project-root \"$root\" --connection local" \
    Enter

  wait_for_pane_text "$tmux_session" "Welcome to Belltower"
  wait_for_pane_text "$tmux_session" "Type to chat"

  send_line "$tmux_session" "/doctor"
  wait_for_pane_text "$tmux_session" "Doctor status=ok"
  send_line "$tmux_session" "/use local bt-tui-mock"
  wait_for_pane_text "$tmux_session" "Using local (bt-tui-mock)"

  for index in $(seq 1 "$turns"); do
    send_line "$tmux_session" "long-session-turn-${index}"
    wait_for_pane_text "$tmux_session" "Mock response for long-session-turn-${index}"
  done

  send_line "$tmux_session" "cancel-hold-turn"
  wait_for_pane_text "$tmux_session" "cancel-hold-turn"
  sleep 0.2
  send_line "$tmux_session" "/cancel"
  wait_for_pane_text_any \
    "$tmux_session" \
    "Cancellation requested for the current turn" \
    "Turn cancelled" \
    "Session cancelled"
  sleep 2.2
  if capture_tmux "$tmux_session" | grep -Fq "late-cancel-output-must-not-appear"; then
    printf 'Cancelled provider output appeared in the TUI transcript.\n' >&2
    return 1
  fi

  send_line "$tmux_session" "steer-hold-turn"
  wait_for_pane_text "$tmux_session" "steer-hold-turn"
  sleep 0.2
  send_line "$tmux_session" "/steer steer-follow-up-token"
  wait_for_pane_text "$tmux_session" "Steering the current turn"
  wait_for_pane_text "$tmux_session" "Initial steer turn completed"
  wait_for_pane_text "$tmux_session" "Steered follow-up completed"

  send_line "$tmux_session" "approval please"
  wait_for_pane_text "$tmux_session" "approval: shell"
  tmux send-keys -t "$tmux_session" Enter
  wait_for_pane_text "$tmux_session" "Approval accepted"

  send_line "$tmux_session" "/inspect session"
  wait_for_pane_text "$tmux_session" "Tool calls:"
  send_line "$tmux_session" "/compact"
  wait_for_pane_text_any \
    "$tmux_session" \
    "Compacted branch" \
    "No context reduction was recorded"
  send_line "$tmux_session" "/export jsonl"
  wait_for_pane_text "$tmux_session" "› /export jsonl"
  tmux send-keys -t "$tmux_session" -l "$temp_dir/session.jsonl"
  tmux send-keys -t "$tmux_session" Enter
  wait_for_pane_text "$tmux_session" "Export jsonl"

  python3 - "$temp_dir/session.jsonl" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
events = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
assert "late-cancel-output-must-not-appear" not in path.read_text()

cancel_seq = next(
    event["seq_id"]
    for event in events
    if "SessionCancelled" in event["payload"]
    and event["payload"]["SessionCancelled"]["reason"] == "cancelled from bt-tui"
)
finished = [
    event
    for event in events
    if "TurnFinished" in event["payload"]
    and event["payload"]["TurnFinished"]["status"] == "cancelled"
    and event["seq_id"] > cancel_seq
]
assert len(finished) == 1, finished
turn_id = finished[0]["payload"]["TurnFinished"]["turn_id"]
assert sum(
    1
    for event in events
    if "TurnFinished" in event["payload"]
    and event["payload"]["TurnFinished"]["turn_id"] == turn_id
) == 1
assert not any(
    event.get("turn_id") == turn_id
    and event["seq_id"] > cancel_seq
    and "MessageAppended" in event["payload"]
    and event["payload"]["MessageAppended"]["message"]["role"] == "assistant"
    for event in events
)
PY

  printf '\n==> TUI tmux acceptance captured pane\n'
  capture_tmux "$tmux_session" | tail -n 80
}

case "${1:-deterministic}" in
  deterministic)
    run_deterministic
    ;;
  tmux)
    run_tmux_smoke
    ;;
  all)
    run_deterministic
    run_tmux_smoke
    ;;
  *)
    printf 'Usage: %s [deterministic|tmux|all]\n' "$0" >&2
    exit 2
    ;;
esac
