#!/usr/bin/env python3
"""A local, isolated mock for LBX Timer's read-only Hubstaff adapter."""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

MOCK_PROJECT_ID = "mock-project-0001"
MOCK_PROJECT_NAME = "Mock Hubstaff project"
STATE_ENV = "LBX_MOCK_HUBSTAFF_STATE"
MODES = ("running", "stopped", "unavailable", "logged-out")


def default_state() -> dict[str, object]:
    return {
        "tracking": False,
        "logged_in": True,
        "ready": True,
        "unavailable": False,
        "active_project": {
            "id": MOCK_PROJECT_ID,
            "name": MOCK_PROJECT_NAME,
            "tracked_today": "0:00:00",
        },
    }


def help_text() -> str:
    return f"""Mock Hubstaff CLI for isolated LBX Timer testing.

The state file is required for every command except --help. Set {STATE_ENV}
to a disposable JSON file path. The script never reads a default file under
HOME and never contacts the network or launches the real Hubstaff CLI.

Controller commands:
  {STATE_ENV}=/tmp/mock-hubstaff.json {sys.argv[0]} set running
  {STATE_ENV}=/tmp/mock-hubstaff.json {sys.argv[0]} set stopped
  {STATE_ENV}=/tmp/mock-hubstaff.json {sys.argv[0]} set unavailable
  {STATE_ENV}=/tmp/mock-hubstaff.json {sys.argv[0]} set logged-out

Read-only adapter commands:
  {sys.argv[0]} send wait-login
  {sys.argv[0]} send status

The read-only commands are exactly the two commands LBX Timer invokes.
All other commands, including start, stop, launch, and resume, are rejected.
"""


def state_path() -> Path:
    raw_path = os.environ.get(STATE_ENV)
    if not raw_path:
        raise ValueError(
            f"{STATE_ENV} must name a disposable mock state file; no default is used."
        )
    return Path(raw_path)


def load_state(path: Path) -> dict[str, object]:
    if not path.exists():
        return default_state()

    try:
        with path.open("r", encoding="utf-8") as state_file:
            state = json.load(state_file)
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"Mock Hubstaff state file is unreadable or malformed: {error}") from error

    if not isinstance(state, dict):
        raise ValueError("Mock Hubstaff state must be a JSON object.")
    for key in ("tracking", "logged_in", "ready", "unavailable"):
        if not isinstance(state.get(key), bool):
            raise ValueError(f"Mock Hubstaff state field {key!r} must be a boolean.")
    project = state.get("active_project")
    if not isinstance(project, dict) or not isinstance(project.get("name"), str):
        raise ValueError("Mock Hubstaff state must include an active_project name.")
    return state


def write_state(path: Path, state: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    file_descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    try:
        with os.fdopen(file_descriptor, "w", encoding="utf-8") as state_file:
            json.dump(state, state_file, separators=(",", ":"), sort_keys=True)
            state_file.write("\n")
            state_file.flush()
            os.fsync(state_file.fileno())
        os.replace(temporary_name, path)
    except Exception:
        try:
            os.unlink(temporary_name)
        except OSError:
            pass
        raise


def set_mode(path: Path, mode: str) -> int:
    if mode not in MODES:
        raise ValueError(f"Unknown mock state {mode!r}; choose one of: {', '.join(MODES)}.")
    state = load_state(path)
    state["tracking"] = mode == "running"
    state["unavailable"] = mode == "unavailable"
    state["logged_in"] = mode != "logged-out"
    state["ready"] = mode != "logged-out"
    write_state(path, state)
    print(f"Mock Hubstaff state set to {mode}.")
    return 0


def send_wait_login(state: dict[str, object]) -> int:
    if state["unavailable"]:
        print("Mock Hubstaff CLI is unavailable.", file=sys.stderr)
        return 2
    print(
        json.dumps(
            {
                "logged_in": state["logged_in"],
                "ready": state["ready"],
                "network_online": False,
            },
            separators=(",", ":"),
        )
    )
    return 0


def send_status(state: dict[str, object]) -> int:
    if state["unavailable"] or not state["logged_in"] or not state["ready"]:
        print("Mock Hubstaff status is unavailable.", file=sys.stderr)
        return 2
    print(
        json.dumps(
            {
                "tracking": state["tracking"],
                "active_project": state["active_project"],
            },
            separators=(",", ":"),
        )
    )
    return 0


def main(arguments: list[str]) -> int:
    if arguments in (["--help"], ["-h"]):
        print(help_text(), end="")
        return 0

    try:
        path = state_path()
        if len(arguments) == 2 and arguments[0] == "set":
            return set_mode(path, arguments[1])

        state = load_state(path)
        if arguments == ["send", "wait-login"]:
            return send_wait_login(state)
        if arguments == ["send", "status"]:
            return send_status(state)
        raise ValueError(
            "Unsupported mock CLI command. Only set <state>, send wait-login, "
            "and send status are accepted."
        )
    except (OSError, ValueError) as error:
        print(f"mock-hubstaff: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
