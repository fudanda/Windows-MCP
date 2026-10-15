"""Optional desktop-manager file transfer bridge; file bytes stay outside MCP."""

import os
from typing import Any, Literal
from urllib.parse import urlsplit

import requests
from mcp.types import ToolAnnotations


def register(mcp: Any, *, get_desktop: Any, get_analytics: Any) -> None:
    """Expose the shared Rust transfer service when launched by the desktop manager."""
    url = os.environ.get("WINDOWS_MCP_TRANSFER_CONTROL_URL", "")
    token = os.environ.get("WINDOWS_MCP_TRANSFER_CONTROL_TOKEN", "")
    if not url or not token:
        return
    parsed = urlsplit(url)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or parsed.path not in ("", "/"):
        raise ValueError("The transfer control bridge must use a loopback HTTP address")

    @mcp.tool(
        name="FileTransfer",
        annotations=ToolAnnotations(
            title="FileTransfer",
            readOnlyHint=False,
            destructiveHint=False,
            idempotentHint=False,
            openWorldHint=True,
        ),
        description=(
            "Manage LAN file/folder transfers using the desktop manager's Rust engine. "
            "For files on the Codex/client computer, no browser or local desktop app is required: "
            "call connection for the transfer port/token and client script path/SHA-256, download "
            "that script from this server's file-service HTTP origin, verify its SHA-256, "
            "then execute it ON THE CLIENT "
            "using `uv run --script windows-mcp-transfer.py --json upload <LOCAL_PATH>` or "
            "`download <REMOTE_PATH> --to <LOCAL_DESTINATION>`. Set WINDOWS_MCP_TRANSFER_URL and "
            "WINDOWS_MCP_TRANSFER_TOKEN in the client process environment, or use --token-stdin. "
            "The script streams HTTP, reports progress, preserves empty directories, and verifies "
            "SHA-256 in both directions. Never put a token in command-line arguments. "
            "The send action below reads paths ON THIS SERVER, not on the Codex/client computer; "
            "add a receiving manager as a peer for server-initiated sends. "
            "Actions: status (settings, saved peer IDs and progress; tokens redacted), connection "
            "(explicitly returns this device's transfer port and token for pairing), configure "
            "(config keys: port, lanEnabled, sharedDir, token; stop receiving and active tasks first), "
            "start/stop (receive service), add_peer (peer keys: name, url http://LAN-IP:port, token), "
            "remove_peer (peer_id), send (peer_id and absolute local paths), cancel (task_id). "
            "Send returns a background task ID. Poll status for byte progress and SHA-256 verified completion. "
            "Receiving never overwrites existing names; folders and empty directories are preserved. "
            "Files use streaming HTTP; do not encode file contents into MCP. "
            "This background tool does not require desktop mouse/keyboard ownership."
        ),
    )
    def file_transfer(
        action: Literal[
            "status",
            "connection",
            "configure",
            "start",
            "stop",
            "add_peer",
            "remove_peer",
            "send",
            "cancel",
        ],
        config: dict[str, Any] | None = None,
        peer: dict[str, str] | None = None,
        peer_id: str | None = None,
        paths: list[str] | None = None,
        task_id: str | None = None,
    ) -> dict[str, Any]:
        """Call the authenticated local control API shared with the Tauri interface."""
        command = {
            "action": action,
            "config": config,
            "peer": peer,
            "peerId": peer_id,
            "paths": paths,
            "taskId": task_id,
        }
        # Ignore system proxy settings for this process-private loopback connection.
        with requests.Session() as session:
            session.trust_env = False
            response = session.post(
                url.rstrip("/") + "/control",
                json=command,
                headers={"Authorization": f"Bearer {token}"},
                timeout=(3, 15),
            )
            if not response.ok:
                try:
                    failure = response.json()
                except ValueError:
                    failure = {"code": "transfer_network", "detail": f"HTTP {response.status_code}"}
                raise RuntimeError(
                    f"{failure.get('code', 'transfer_network')}: {failure.get('detail', '')}"
                )
            return response.json()
