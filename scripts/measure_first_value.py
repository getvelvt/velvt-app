#!/usr/bin/env python3
"""Measure the isolated helper handshake and first privacy-safe Today signal.

The harness creates its own temporary socket and SQLite database. It never
reads or mutates the installed app's database, account, defaults, permissions,
or Docker environment, and it does not print the synthetic raw test activity.
"""

from __future__ import annotations

import argparse
from contextlib import nullcontext
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
from datetime import datetime, timezone
from uuid import uuid4


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
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--taxonomy", type=Path, required=True)
    parser.add_argument("--qualifying-seconds", type=int, default=60)
    parser.add_argument("--timeout-seconds", type=int, default=90)
    parser.add_argument(
        "--work-dir",
        type=Path,
        help="Use an existing isolated directory instead of creating a temporary one.",
    )
    return parser.parse_args()


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
            raise RuntimeError(f"local dashboard leaked forbidden keys: {sorted(forbidden)}")
        for value in payload.values():
            assert_private_payload(value)
    elif isinstance(payload, list):
        for value in payload:
            assert_private_payload(value)


def wait_for_socket(path: Path, process: subprocess.Popen, deadline: float) -> None:
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("local service exited before creating its socket")
        if path.exists():
            return
        time.sleep(0.01)
    raise TimeoutError("local service socket was not ready before the deadline")


def main() -> int:
    args = parse_args()
    if args.qualifying_seconds < 60:
        raise SystemExit("--qualifying-seconds must be at least 60")
    if not args.helper.is_file() or not os.access(args.helper, os.X_OK):
        raise SystemExit(f"helper is missing or not executable: {args.helper}")
    if not args.taxonomy.is_file():
        raise SystemExit(f"taxonomy is missing: {args.taxonomy}")

    started = time.monotonic()
    deadline = started + args.timeout_seconds
    if args.work_dir is not None:
        args.work_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        directory_context = nullcontext(str(args.work_dir.resolve()))
    else:
        directory_context = tempfile.TemporaryDirectory(prefix="velvt-first-value-")
    with directory_context as directory:
        root = Path(directory)
        socket_path = root / "service.sock"
        database_path = root / "service.sqlite3"
        # Do not inherit developer overrides: an unrelated invalid VELVT_*
        # setting would make this clean-state measurement nondeterministic.
        environment = {
            "HOME": os.environ.get("HOME", str(root)),
            "PATH": os.environ.get("PATH", "/usr/bin:/bin:/usr/sbin:/sbin"),
            "TMPDIR": str(root),
            "VELVT_IPC_SOCKET_PATH": str(socket_path),
            "VELVT_DATABASE_PATH": str(database_path),
            "VELVT_ABSTRACTION_TAXONOMY_PATH": str(args.taxonomy),
            "VELVT_API_BASE_URL": "http://127.0.0.1:65534",
            "VELVT_LOG_LEVEL": "error",
        }
        process = subprocess.Popen(
            [str(args.helper)],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        metrics: dict[str, float | int | bool] = {
            "helper_process_seconds": round(time.monotonic() - started, 3)
        }
        connection = None
        try:
            wait_for_socket(socket_path, process, deadline)
            metrics["socket_ready_seconds"] = round(time.monotonic() - started, 3)
            connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            connection.settimeout(max(1.0, deadline - time.monotonic()))
            connection.connect(str(socket_path))
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
                        "client_version": "first-value-verification",
                    },
                },
            )
            acknowledged = receive(stream)
            if acknowledged.get("type") != "acknowledged":
                raise RuntimeError("local service did not confirm the handshake")
            metrics["handshake_seconds"] = round(time.monotonic() - started, 3)
            metrics["protocol_version"] = protocol

            qualification_at = started + args.qualifying_seconds
            while time.monotonic() < qualification_at:
                time.sleep(min(0.25, qualification_at - time.monotonic()))

            occurred_at = datetime.now(timezone.utc).timestamp() - args.qualifying_seconds
            send(
                stream,
                {
                    "type": "raw_event",
                    "payload": {
                        "event_id": str(uuid4()),
                        "occurred_at": datetime.fromtimestamp(
                            occurred_at, timezone.utc
                        ).isoformat().replace("+00:00", "Z"),
                        "duration_seconds": args.qualifying_seconds,
                        "app_name": "Visual Studio Code",
                        "window_title": "first-value-verification",
                        "bundle_id": "com.microsoft.VSCode",
                    },
                },
            )
            metrics["activity_qualified_seconds"] = round(
                time.monotonic() - started, 3
            )

            ready_payload = None
            while time.monotonic() < deadline:
                message = receive(stream)
                if message.get("type") == "local_dashboard":
                    payload = message["payload"]
                    assert_private_payload(payload)
                    if payload["early_signal"]["status"] == "ready":
                        ready_payload = payload
                        break
            if ready_payload is None:
                raise TimeoutError("early local signal was not ready before the deadline")

            metrics["today_ready_seconds"] = round(time.monotonic() - started, 3)
            metrics["observed_seconds"] = ready_payload["early_signal"][
                "observed_seconds"
            ]
            metrics["privacy_scan_passed"] = True

            # Re-requesting during the deliberately unreachable backend proves
            # local value remains available independently of synchronization.
            send(
                stream,
                {
                    "type": "request_local_dashboard",
                    "payload": {"window_seconds": 3600},
                },
            )
            while time.monotonic() < deadline:
                message = receive(stream)
                if message.get("type") == "local_dashboard":
                    assert_private_payload(message["payload"])
                    metrics["offline_local_signal_preserved"] = (
                        message["payload"]["early_signal"]["status"] == "ready"
                    )
                    break
            if not metrics.get("offline_local_signal_preserved"):
                raise RuntimeError("local signal was not preserved while backend was offline")

            print(json.dumps(metrics, indent=2, sort_keys=True))
            return 0
        finally:
            if connection is not None:
                connection.close()
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


if __name__ == "__main__":
    raise SystemExit(main())
