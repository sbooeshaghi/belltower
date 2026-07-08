#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if [[ "${BELLTOWER_LIVE_ACCEPTANCE:-0}" != "1" ]]; then
  printf 'SKIP live acceptance: set BELLTOWER_LIVE_ACCEPTANCE=1 to run provider-backed gates.\n'
  exit 0
fi

mode="${1:-all}"
temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/belltower-live-acceptance.XXXXXX")"
server_log="$temp_dir/bt-server.log"
server_pid=""

cleanup() {
  if [[ -n "${server_pid:-}" ]]; then
    kill "$server_pid" >/dev/null 2>&1 || true
    wait "$server_pid" 2>/dev/null || true
  fi
  if [[ -n "${temp_dir:-}" ]]; then rm -rf "$temp_dir"; fi
}
trap cleanup EXIT

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

json_field() {
  local path="$1"
  python3 -c '
import json
import sys

path = sys.argv[1].split(".")
value = json.load(sys.stdin)
for key in path:
    value = value[key]
print(value)
' "$path"
}

start_server() {
  local port="$1"
  local -a env_prefix=()
  if [[ -n "${BELLTOWER_LIVE_CONFIG_DIR:-}" ]]; then
    env_prefix+=(BELLTOWER_CONFIG_DIR="$BELLTOWER_LIVE_CONFIG_DIR")
  fi
  if [[ -n "${BELLTOWER_LIVE_DATA_DIR:-}" ]]; then
    env_prefix+=(BELLTOWER_DATA_DIR="$BELLTOWER_LIVE_DATA_DIR")
  fi

  if (( ${#env_prefix[@]} > 0 )); then
    env "${env_prefix[@]}" cargo run -p bt-server -- \
      --host 127.0.0.1 \
      --port "$port" \
      --database "$temp_dir/belltower-live.sqlite" >"$server_log" 2>&1 &
  else
    cargo run -p bt-server -- \
      --host 127.0.0.1 \
      --port "$port" \
      --database "$temp_dir/belltower-live.sqlite" >"$server_log" 2>&1 &
  fi
  server_pid="$!"
  if ! wait_for_url "http://127.0.0.1:${port}/health" "bt-server"; then
    printf '\n==> bt-server log\n' >&2
    cat "$server_log" >&2 || true
    return 1
  fi
}

bt_tui() {
  local server_url="$1"
  shift
  local -a env_prefix=()
  if [[ -n "${BELLTOWER_LIVE_CONFIG_DIR:-}" ]]; then
    env_prefix+=(BELLTOWER_CONFIG_DIR="$BELLTOWER_LIVE_CONFIG_DIR")
  fi
  if [[ -n "${BELLTOWER_LIVE_DATA_DIR:-}" ]]; then
    env_prefix+=(BELLTOWER_DATA_DIR="$BELLTOWER_LIVE_DATA_DIR")
  fi
  if (( ${#env_prefix[@]} > 0 )); then
    env "${env_prefix[@]}" cargo run -p bt-tui -- --server "$server_url" "$@"
  else
    cargo run -p bt-tui -- --server "$server_url" "$@"
  fi
}

run_turn() {
  local label="$1"
  local connection="$2"
  local expected="$3"
  local server_url="$4"

  printf '\n==> %s: create session on %s\n' "$label" "$connection"
  local created
  created="$(bt_tui "$server_url" sessions create "$root" --connection "$connection" --display-name "$label")"
  local session_id branch_id
  session_id="$(printf '%s' "$created" | json_field session.session_id)"
  branch_id="$(printf '%s' "$created" | json_field branch.branch_id)"

  printf '==> %s: send first turn\n' "$label"
  bt_tui "$server_url" send "$session_id" "$branch_id" \
    "Reply with exactly this token and no extra text: ${expected}" >/dev/null

  for _ in $(seq 1 "${BELLTOWER_LIVE_ACCEPTANCE_POLL_ATTEMPTS:-80}"); do
    local messages
    messages="$(bt_tui "$server_url" messages "$session_id" || true)"
    if printf '%s' "$messages" | grep -Fq "$expected"; then
      printf '==> %s: observed expected token %s\n' "$label" "$expected"
      return 0
    fi
    sleep "${BELLTOWER_LIVE_ACCEPTANCE_POLL_SECONDS:-1}"
  done

  printf 'Live acceptance turn did not observe expected token for %s.\n' "$label" >&2
  printf 'Session: %s\nBranch: %s\nExpected: %s\n' "$session_id" "$branch_id" "$expected" >&2
  return 1
}

server_port="$(free_port)"
server_url="http://127.0.0.1:${server_port}/"
start_server "$server_port"

case "$mode" in
  local)
    run_turn "M1 local model happy path" "${BELLTOWER_LIVE_LOCAL_CONNECTION:-local}" \
      "${BELLTOWER_LIVE_LOCAL_EXPECTED:-belltower-live-local-ok}" "$server_url"
    ;;
  cloud)
    run_turn "M2 cloud provider happy path" "${BELLTOWER_LIVE_CLOUD_CONNECTION:-chatgpt}" \
      "${BELLTOWER_LIVE_CLOUD_EXPECTED:-belltower-live-cloud-ok}" "$server_url"
    ;;
  all)
    run_turn "M1 local model happy path" "${BELLTOWER_LIVE_LOCAL_CONNECTION:-local}" \
      "${BELLTOWER_LIVE_LOCAL_EXPECTED:-belltower-live-local-ok}" "$server_url"
    run_turn "M2 cloud provider happy path" "${BELLTOWER_LIVE_CLOUD_CONNECTION:-chatgpt}" \
      "${BELLTOWER_LIVE_CLOUD_EXPECTED:-belltower-live-cloud-ok}" "$server_url"
    ;;
  *)
    printf 'Usage: %s [local|cloud|all]\n' "$0" >&2
    exit 2
    ;;
esac
