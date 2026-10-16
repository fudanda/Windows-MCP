"""Portable helper contracts; no Windows backend or remote device is required."""

import asyncio
import base64
import hashlib
import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import httpx2
import pytest

SCRIPT = Path(__file__).parents[1] / "windows-mcp/scripts/windows_mcp_client.py"
SPEC = importlib.util.spec_from_file_location("windows_mcp_skill_client", SCRIPT)
client = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(client)


def result(value: Any, *, error: bool = False) -> SimpleNamespace:
    """Build the MCP SDK fields consumed by the helper."""
    return SimpleNamespace(is_error=error, structured_content=value, content=[])


@pytest.mark.parametrize(
    "endpoint,origin",
    [
        ("http://192.168.10.20:8001/mcp", "http://192.168.10.20:9002"),
        ("http://localhost:8001/mcp", "http://localhost:9002"),
        ("http://[fd00::1]:8001/mcp", "http://[fd00::1]:9002"),
    ],
)
def test_bootstrap_uses_selected_host(endpoint: str, origin: str) -> None:
    assert client.transfer_origin(endpoint, 9002) == origin


@pytest.mark.parametrize(
    "endpoint,port",
    [
        ("http://8.8.8.8/mcp", 8002),
        ("http://example.com/mcp", 8002),
        ("http://192.168.1.2/mcp", True),
        ("http://192.168.1.2/mcp", 0),
        ("http://192.168.1.2/mcp", "8002"),
        ("http://192.168.1.2/mcp", 65536),
    ],
)
def test_bootstrap_rejects_invalid_host_or_port(endpoint: str, port: Any) -> None:
    with pytest.raises(ValueError):
        client.transfer_origin(endpoint, port)


@pytest.mark.parametrize(
    "endpoint",
    [
        "file:///tmp/mcp",
        "http://user:secret@localhost/mcp",
        "http://localhost/mcp?token=x",
        "http://localhost/mcp#secret",
        "http://localhost:0/mcp",
    ],
)
def test_endpoint_rejects_credentials_and_non_http_urls(endpoint: str) -> None:
    with pytest.raises(ValueError):
        client.mcp_url(endpoint)


def test_redacts_structured_and_nested_json_credentials() -> None:
    token = 'printable-credential-with-quote-"-and-backslash-\\'
    payload = {"result": {"token": token}, "text": [json.dumps({"token": token})]}
    secrets: set[str] = set()
    sanitized = json.loads(client.redact(payload, secrets))
    assert sanitized["result"]["token"] == "[REDACTED]"
    assert json.loads(sanitized["text"][0])["token"] == "[REDACTED]"
    assert token in secrets
    assert "credential" not in json.dumps(sanitized)


def test_redacts_known_credentials_in_error_text() -> None:
    token = "an-existing-session-credential"
    output = json.loads(client.redact({"detail": f"Authentication failed for {token}"}, {token}))
    assert output["detail"] == "Authentication failed for [REDACTED]"


def test_image_blocks_are_saved_without_base64_stdout(tmp_path: Path) -> None:
    content = b"fixture image bytes"
    image = SimpleNamespace(type="image", data=base64.b64encode(content), mime_type="image/png")
    response = result(None)
    response.content = [image]
    first = client.summarize(response, tmp_path)
    second = client.summarize(response, tmp_path)
    path = Path(first["images"][0]["path"])
    assert path.is_absolute() and path.read_bytes() == content
    assert path != Path(second["images"][0]["path"])
    assert "data" not in first["images"][0]


def test_tool_errors_are_not_unpacked_as_success() -> None:
    response = result(None, error=True)
    response.content = [SimpleNamespace(type="text", text="Stopped")]
    with pytest.raises(RuntimeError, match="Stopped"):
        client.unpack(response)


@pytest.mark.parametrize("case", ["valid", "mismatch", "oversize", "redirect"])
def test_download_verifies_hash_and_size_before_execution(case: str) -> None:
    content = b"print('verified client')\n"
    requests = []

    def respond(request: httpx2.Request) -> httpx2.Response:
        requests.append(request)
        if case == "redirect":
            return httpx2.Response(302, headers={"Location": "http://192.168.1.99/client.py"})
        return httpx2.Response(
            200, content=b"x" * (client.MAX_CLIENT_BYTES + 1) if case == "oversize" else content
        )

    async def check() -> bytes:
        async with httpx2.AsyncClient(
            transport=httpx2.MockTransport(respond), follow_redirects=False
        ) as http:
            return await client.download_client(
                http,
                "http://192.168.1.2:8002",
                {
                    "path": "/client/windows-mcp-transfer.py",
                    "sha256": "0" * 64
                    if case == "mismatch"
                    else hashlib.sha256(content).hexdigest(),
                },
            )

    if case == "valid":
        assert asyncio.run(check()) == content
    else:
        with pytest.raises((ValueError, httpx2.HTTPStatusError)):
            asyncio.run(check())
    assert len(requests) == 1
    assert "authorization" not in requests[0].headers


@pytest.mark.parametrize("argument", ["--url=x", "--ur=x", "--token-file", "--token-f=x"])
def test_forwarded_credentials_cannot_override_connection(argument: str) -> None:
    with pytest.raises(ValueError, match="selected MCP connection"):
        asyncio.run(client.transfer(None, "http://192.168.1.2/mcp", [argument], 10, set()))


def test_transfer_keeps_cwd_and_uses_only_child_credentials(monkeypatch: Any) -> None:
    token = "transfer-only-token-of-at-least-32-characters"
    script_bytes = b"# test-only transfer client\n"
    calls = []
    executed = []

    class Session:
        async def call_tool(self, name: str, args: dict) -> SimpleNamespace:
            calls.append((name, args))
            if args["action"] == "status":
                return result({"running": True})
            return result(
                {
                    "token": token,
                    "port": 8002,
                    "client": {
                        "path": "/client/windows-mcp-transfer.py",
                        "sha256": hashlib.sha256(script_bytes).hexdigest(),
                    },
                }
            )

    async def download(http: Any, origin: str, metadata: dict) -> bytes:
        assert "authorization" not in http.headers
        assert origin == "http://192.168.1.2:8002"
        return script_bytes

    def execute(command: list[str], **kwargs: Any) -> int:
        assert "cwd" not in kwargs
        assert kwargs["env"]["WINDOWS_MCP_TRANSFER_TOKEN"] == token
        assert kwargs["env"]["WINDOWS_MCP_TRANSFER_URL"] == "http://192.168.1.2:8002"
        assert "WINDOWS_MCP_TOKEN" not in kwargs["env"]
        assert token not in " ".join(command)
        assert command[:4] == ["uv", "run", "--no-project", "--python"]
        assert command[4] == client.sys.executable and command[5] == "--script"
        script = Path(command[6])
        assert script.read_bytes() == script_bytes
        assert command[7:] == ["--json", "upload", "./dist"]
        executed.append(script)
        return 7

    monkeypatch.setenv("WINDOWS_MCP_TOKEN", "parent-credential")
    monkeypatch.setattr(client, "download_client", download)
    monkeypatch.setattr(client.subprocess, "call", execute)
    code = asyncio.run(
        client.transfer(
            Session(),
            "http://192.168.1.2:8001/mcp",
            ["--", "--json", "upload", "./dist"],
            10,
            set(),
        )
    )
    assert code == 7
    assert [args["action"] for _, args in calls] == ["status", "connection"]
    assert len(executed) == 1 and not executed[0].exists()


def test_stopped_service_does_not_get_started() -> None:
    class Session:
        async def call_tool(self, name: str, args: dict) -> SimpleNamespace:
            assert name == "FileTransfer" and args == {"action": "status"}
            return result({"running": False})

    with pytest.raises(ValueError, match="File service is stopped"):
        asyncio.run(
            client.transfer(
                Session(), "http://192.168.1.2:8001/mcp", ["--json", "status"], 10, set()
            )
        )
