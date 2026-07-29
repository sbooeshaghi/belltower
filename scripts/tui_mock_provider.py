#!/usr/bin/env python3
"""Small OpenAI-compatible streaming mock for scripted TUI acceptance."""

from __future__ import annotations

import argparse
import json
import signal
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


MODEL_ID = "bt-tui-mock"


def _json_response(handler: BaseHTTPRequestHandler, payload: object) -> None:
    body = json.dumps(payload).encode("utf-8")
    handler.send_response(200)
    handler.send_header("content-type", "application/json")
    handler.send_header("content-length", str(len(body)))
    handler.end_headers()
    handler.wfile.write(body)


def _sse_event(payload: object) -> bytes:
    return f"data: {json.dumps(payload, separators=(',', ':'))}\n\n".encode("utf-8")


def _last_user_text(messages: list[dict[str, object]]) -> str:
    for message in reversed(messages):
        if message.get("role") != "user":
            continue
        content = message.get("content")
        if isinstance(content, str):
            return content
        if isinstance(content, list):
            return " ".join(
                part.get("text", "")
                for part in content
                if isinstance(part, dict)
            )
        return str(content or "")
    return ""


def _has_tool_result(messages: list[dict[str, object]]) -> bool:
    return any(message.get("role") == "tool" for message in messages)


def _text_chunks(text: str) -> list[object]:
    midpoint = max(1, len(text) // 2)
    return [
        {"choices": [{"delta": {"content": text[:midpoint]}, "finish_reason": None}]},
        {"choices": [{"delta": {"content": text[midpoint:]}, "finish_reason": None}]},
        {
            "choices": [],
            "usage": {
                "prompt_tokens": 12,
                "completion_tokens": 8,
                "total_tokens": 20,
            },
        },
    ]


def _tool_call_chunks() -> list[object]:
    arguments = json.dumps(
        {
            "command": "printf tui-approval",
            "call_id": "tui-approval",
        },
        separators=(",", ":"),
    )
    return [
        {
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call-tui-approval",
                                "function": {
                                    "name": "shell",
                                    "arguments": arguments,
                                },
                            }
                        ]
                    },
                    "finish_reason": "tool_calls",
                }
            ]
        }
    ]


class Handler(BaseHTTPRequestHandler):
    server_version = "BelltowerTuiMock/1.0"

    def log_message(self, fmt: str, *args: object) -> None:
        print(fmt % args, file=sys.stderr, flush=True)

    def do_GET(self) -> None:
        if self.path.rstrip("/") == "/v1/models":
            _json_response(self, {"data": [{"id": MODEL_ID, "object": "model"}]})
            return
        self.send_error(404, "not found")

    def do_POST(self) -> None:
        if self.path.rstrip("/") != "/v1/chat/completions":
            self.send_error(404, "not found")
            return

        length = int(self.headers.get("content-length", "0"))
        payload = json.loads(self.rfile.read(length).decode("utf-8") or "{}")
        messages = payload.get("messages") or []
        if not isinstance(messages, list):
            messages = []
        last_user = _last_user_text(messages)

        delay_before_chunks = 0.0
        if _has_tool_result(messages):
            chunks = _text_chunks("Approval accepted. Tool result recorded in history.\n")
        elif "approval" in last_user.lower():
            chunks = _tool_call_chunks()
        elif "cancel-hold-turn" in last_user:
            delay_before_chunks = 2.0
            chunks = _text_chunks("late-cancel-output-must-not-appear\n")
        elif "steer-hold-turn" in last_user:
            delay_before_chunks = 0.75
            chunks = _text_chunks("Initial steer turn completed.\n")
        elif "steer-follow-up-token" in last_user:
            chunks = _text_chunks("Steered follow-up completed.\n")
        elif "long-session-turn" in last_user:
            chunks = _text_chunks(f"Mock response for {last_user.strip()}.\n")
        else:
            chunks = _text_chunks("Mock response from the scripted TUI provider.\n")

        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.end_headers()
        if delay_before_chunks:
            time.sleep(delay_before_chunks)
        try:
            for chunk in chunks:
                self.wfile.write(_sse_event(chunk))
                self.wfile.flush()
                time.sleep(0.02)
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            print(f"client disconnected before response for {last_user!r}", file=sys.stderr, flush=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--port-file", required=True)
    args = parser.parse_args()

    server = ThreadingHTTPServer((args.host, args.port), Handler)
    with open(args.port_file, "w", encoding="utf-8") as handle:
        handle.write(str(server.server_address[1]))

    def stop(_signum: int, _frame: object) -> None:
        # HTTPServer.shutdown() must be called from a different thread than
        # serve_forever(); calling it directly from this main-thread signal
        # handler deadlocks acceptance teardown. Unwind instead and close the
        # listening socket in the normal finally path.
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        server.serve_forever()
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
