#!/usr/bin/env python3
"""Measure a state-preserving warm launch of the packaged macOS app.

This probe does not clear application state, remove sockets, or alter defaults.
It refuses to start when Velvt or its helper is already running and leaves the
launched app running so the caller can terminate it through NSRunningApplication.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import time


FORBIDDEN_KEYS = {
    "app_name",
    "window_title",
    "bundle_id",
    "url",
    "filename",
    "path",
    "contact",
    "intention",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument(
        "--socket",
        type=Path,
        default=Path.home() / ".velvt" / "velvt-service.sock",
    )
    parser.add_argument("--timeout-seconds", type=float, default=15.0)
    parser.add_argument(
        "--attach",
        action="store_true",
        help="Probe an already-running app/helper without launching another instance.",
    )
    return parser.parse_args()


def matching_pids(name: str) -> list[int]:
    result = subprocess.run(
        ["pgrep", "-x", name], capture_output=True, text=True, check=False
    )
    if result.returncode not in (0, 1):
        raise RuntimeError(f"pgrep failed for {name}: {result.stderr.strip()}")
    return [int(value) for value in result.stdout.split()]


def receive(stream) -> dict:
    line = stream.readline()
    if not line:
        raise RuntimeError("local service closed the IPC connection")
    return json.loads(line)


def send(stream, message: dict) -> None:
    stream.write(json.dumps(message, separators=(",", ":")) + "\n")
    stream.flush()


def assert_private_payload(payload: object) -> None:
    if isinstance(payload, dict):
        forbidden = FORBIDDEN_KEYS.intersection(payload)
        if forbidden:
            raise RuntimeError(f"dashboard leaked forbidden keys: {sorted(forbidden)}")
        for value in payload.values():
            assert_private_payload(value)
    elif isinstance(payload, list):
        for value in payload:
            assert_private_payload(value)


def main() -> int:
    args = parse_args()
    executable = args.app / "Contents" / "MacOS" / "Velvt"
    helper = args.app / "Contents" / "Resources" / "velvt-service"
    if not executable.is_file() or not os.access(executable, os.X_OK):
        raise SystemExit(f"packaged app executable is unavailable: {executable}")
    if not helper.is_file() or not os.access(helper, os.X_OK):
        raise SystemExit(f"packaged helper is unavailable: {helper}")
    started = time.monotonic()
    deadline = started + args.timeout_seconds
    metrics: dict[str, object] = {}
    app_pids = matching_pids("Velvt")
    helper_pids = matching_pids("velvt-service")
    if args.attach:
        if not app_pids or not helper_pids:
            raise SystemExit("--attach requires an already-running app and helper")
    else:
        if app_pids or helper_pids:
            raise SystemExit("Velvt or velvt-service is already running; refusing to launch")
        subprocess.run(["open", str(args.app.resolve())], check=True)

    while not app_pids and time.monotonic() < deadline:
        app_pids = matching_pids("Velvt")
        if not app_pids:
            time.sleep(0.01)
    if not app_pids:
        raise TimeoutError("app process did not appear before the deadline")
    metrics["app_process_seconds"] = round(time.monotonic() - started, 3)

    while not helper_pids and time.monotonic() < deadline:
        helper_pids = matching_pids("velvt-service")
        if not helper_pids:
            time.sleep(0.01)
    if not helper_pids:
        raise TimeoutError("helper process did not appear before the deadline")
    metrics["helper_process_seconds"] = round(time.monotonic() - started, 3)

    connection: socket.socket | None = None
    while time.monotonic() < deadline:
        candidate = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        candidate.settimeout(max(0.1, deadline - time.monotonic()))
        try:
            candidate.connect(str(args.socket.expanduser()))
        except OSError:
            candidate.close()
            time.sleep(0.01)
            continue
        connection = candidate
        metrics["socket_connect_seconds"] = round(time.monotonic() - started, 3)
        break
    if connection is None:
        raise TimeoutError("helper socket was not connectable before the deadline")

    try:
        stream = connection.makefile("rw", encoding="utf-8", newline="\n")
        hello = receive(stream)
        if hello.get("type") != "server_hello":
            raise RuntimeError("local service did not begin with server_hello")
        protocol = hello["payload"]["protocol_version"]
        send(
            stream,
            {
                "type": "client_hello",
                "payload": {
                    "expected_protocol_version": protocol,
                    "client_version": "warm-reopen-verification",
                },
            },
        )
        while time.monotonic() < deadline:
            message = receive(stream)
            if message.get("type") == "acknowledged":
                metrics["handshake_seconds"] = round(time.monotonic() - started, 3)
                metrics["protocol_version"] = protocol
                break
        if "handshake_seconds" not in metrics:
            raise TimeoutError("protocol handshake was not acknowledged")

        send(
            stream,
            {"type": "request_local_dashboard", "payload": {"window_seconds": 3600}},
        )
        while time.monotonic() < deadline:
            message = receive(stream)
            if message.get("type") == "local_dashboard":
                payload = message["payload"]
                assert_private_payload(payload)
                metrics["today_response_seconds"] = round(
                    time.monotonic() - started, 3
                )
                metrics["early_signal_status"] = payload["early_signal"]["status"]
                metrics["privacy_scan_passed"] = True
                break
        if "today_response_seconds" not in metrics:
            raise TimeoutError("local Today response did not arrive before the deadline")
    finally:
        connection.close()

    metrics["app_pid"] = app_pids[0]
    metrics["helper_pid"] = helper_pids[0]
    print(json.dumps(metrics, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
