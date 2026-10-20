"""Current-user, expiring desktop handoff. No credentials or remote input API."""

from __future__ import annotations

import json
import os
from pathlib import Path
import threading
import time


class DesktopHandoff:
    def __init__(self, directory: Path | None = None):
        self.directory = (
            directory
            or Path(os.environ.get("LOCALAPPDATA", Path.home()))
            / "Windows-MCP"
            / "desktop-control-v1"
        )
        self.acknowledged_owner: str | None = None
        self.thread: threading.Thread | None = None

    def request_owner(self) -> str | None:
        try:
            path = self.directory / "request.json"
            if path.stat().st_size > 1024:
                return None
            data = json.loads(path.read_text(encoding="utf-8"))
            if not isinstance(data, dict) or data.get("version") != 1:
                return None
            owner = data.get("ownerId")
            stamp = data.get("updatedAt")
            if not isinstance(owner, str) or not 1 <= len(owner) <= 128 or not owner.isascii():
                return None
            if isinstance(stamp, bool) or not isinstance(stamp, (float, int)):
                return None
            age = time.time() * 1000 - stamp
            return owner if -500 <= age <= 5000 else None
        except OSError, ValueError, TypeError:
            return None

    def publish(self, status: dict, owner: str | None, last_input: float) -> dict:
        now = time.monotonic()
        snapshot = {
            "version": 1,
            "pid": os.getpid(),
            "updatedAt": time.time() * 1000,
            "state": status["state"],
            "generation": status["generation"],
            "acknowledgedOwner": owner,
            "lastPhysicalInputAt": (time.time() - max(0, now - last_input)) * 1000
            if last_input
            else None,
        }
        self.directory.mkdir(parents=True, exist_ok=True)
        temporary = self.directory / f"status-{os.getpid()}.tmp"
        temporary.write_text(json.dumps(snapshot), encoding="utf-8")
        os.replace(temporary, self.directory / "status.json")
        return snapshot

    def start(self, controller) -> None:
        def run():
            while not controller._stop.is_set():
                try:
                    controller.publish_handoff()
                except OSError:
                    pass  # The robot refuses stale/unreadable acknowledgements.
                controller._stop.wait(0.25)

        if self.thread and self.thread.is_alive():
            return
        self.thread = threading.Thread(target=run, name="windows-mcp-handoff", daemon=True)
        self.thread.start()
