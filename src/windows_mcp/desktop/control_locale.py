"""Read the manager's saved language without restarting desktop control."""

import json
import os
from pathlib import Path
import time


class NoticeLanguage:
    """Keep the last valid language through atomic saves or transient read errors."""

    def __init__(self) -> None:
        self.language = "en" if os.environ.get("WINDOWS_MCP_LANGUAGE") == "en" else "zh-CN"
        config = os.environ.get("WINDOWS_MCP_MANAGER_CONFIG")
        self.config = Path(config) if config else None
        self.next_read = 0.0

    def current(self) -> str:
        """Poll only the language field, at most twice per second while active."""
        now = time.monotonic()
        if self.config is not None and now >= self.next_read:
            self.next_read = now + 0.5
            try:
                value = json.loads(self.config.read_text(encoding="utf-8-sig"))
                language = value.get("language") if isinstance(value, dict) else None
                if language in ("zh-CN", "en"):
                    self.language = language
            except OSError, ValueError:
                # The manager may be replacing the file; never break the control
                # indicator or expose the rest of its configuration in a log.
                pass
        return self.language
