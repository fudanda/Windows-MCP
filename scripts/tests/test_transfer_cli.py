"""Exercise the standalone client without importing the Windows desktop backend."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

import httpx
import pytest

SPEC = importlib.util.spec_from_file_location(
    "transfer_cli", Path(__file__).parents[1] / "windows-mcp-transfer.py"
)
assert SPEC and SPEC.loader
cli = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = cli
SPEC.loader.exec_module(cli)
TOKEN = "test-token-" + "x" * 40


class MemoryServer:
    def __init__(self) -> None:
        self.directories = {""}
        self.files: dict[str, bytes] = {}
        self.bad_hash = False
        self.interrupted = False
        self.requests: list[tuple[str, str]] = []

    def handle(self, request: httpx.Request) -> httpx.Response:
        assert request.headers["authorization"] == f"Bearer {TOKEN}"
        self.requests.append((request.method, request.url.path))
        if request.url.path == "/v1/info":
            return httpx.Response(200, json={"protocol": "windows-mcp-transfer/1"})
        path = request.url.path.removeprefix("/files/").rstrip("/")
        if request.method == "HEAD":
            return httpx.Response(200 if path in self.files or path in self.directories else 404)
        if request.method == "MKCOL":
            if path in self.directories or path in self.files:
                return httpx.Response(409)
            self.directories.add(path)
            return httpx.Response(201)
        if request.method == "PUT":
            assert request.headers["content-length"] == str(len(request.content))
            assert "transfer-encoding" not in request.headers
            digest = hashlib.sha256(request.content).hexdigest()
            assert request.headers["x-content-sha256"] == digest
            self.files[path] = request.content
            return httpx.Response(201, json={"verified": True, "sha256": digest, "taskId": "task"})
        if request.url.query == b"json":
            if path not in self.directories:
                return httpx.Response(404)
            prefix = path + "/" if path else ""
            paths = []
            for name in sorted(self.directories | self.files.keys()):
                if name == path or not name.startswith(prefix):
                    continue
                child = name[len(prefix) :]
                if "/" not in child:
                    directory = name in self.directories
                    paths.append(
                        {
                            "name": child,
                            "path_type": "Dir" if directory else "File",
                            "size": 0 if directory else len(self.files[name]),
                        }
                    )
            return httpx.Response(200, json={"kind": "Index", "dir_exists": True, "paths": paths})
        if request.url.query == b"hash":
            digest = "0" * 64 if self.bad_hash else hashlib.sha256(self.files[path]).hexdigest()
            return httpx.Response(200, text=digest)
        if self.interrupted:

            class Interrupted(httpx.SyncByteStream):
                def __iter__(self):
                    yield b"partial"
                    raise KeyboardInterrupt

            return httpx.Response(200, stream=Interrupted())
        return httpx.Response(200, content=self.files[path])


def client(server: MemoryServer) -> Any:
    return cli.Client(
        "http://127.0.0.1:8002",
        TOKEN,
        cli.Reporter(json_lines=True),
        transport=httpx.MockTransport(server.handle),
    )


def test_folder_roundtrip_unicode_empty_file_and_empty_directory(tmp_path, capsys):
    source = tmp_path / "中文目录"
    (source / "empty").mkdir(parents=True)
    (source / "nested").mkdir()
    (source / "nested" / "内容.txt").write_bytes(b"payload" * 100_000)
    (source / "zero").touch()
    server = MemoryServer()
    with client(server) as transfer:
        transfer.connect()
        result = transfer.upload(source, "")
        assert result["sha256Verified"] and result["files"] == 2
    with client(server) as transfer:
        target = tmp_path / "received"
        result = transfer.download(source.name, target)
        assert result["sha256Verified"] and result["files"] == 2
    assert (target / "empty").is_dir()
    for file in source.rglob("*"):
        if file.is_file():
            assert (target / file.relative_to(source)).read_bytes() == file.read_bytes()
    events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    assert any(e["event"] == "progress" and e["transferredBytes"] > 0 for e in events)
    assert len([e for e in events if e["event"] == "verified"]) == 4


def test_hash_failure_never_publishes_download_and_cleans_stage(tmp_path):
    server = MemoryServer()
    server.files["file.bin"] = b"original content"
    server.bad_hash = True
    target = tmp_path / "received.bin"
    with client(server) as transfer, pytest.raises(cli.TransferError) as failure:
        transfer.download("file.bin", target)
    assert failure.value.code == "transfer_integrity"
    assert not target.exists()
    assert not list(tmp_path.glob(".wmcp-cli-*"))


def test_interrupted_download_cleans_private_file(tmp_path):
    server = MemoryServer()
    server.files["file.bin"] = b"original content" * 100
    server.interrupted = True
    target = tmp_path / "received.bin"
    with client(server) as transfer, pytest.raises(KeyboardInterrupt):
        transfer.download("file.bin", target)
    assert not target.exists()
    assert not list(tmp_path.glob(".wmcp-cli-*"))


def test_existing_local_and_remote_destinations_are_preserved(tmp_path):
    source = tmp_path / "file.txt"
    source.write_bytes(b"source")
    target = tmp_path / "existing"
    target.write_bytes(b"keep")
    server = MemoryServer()
    server.files["file.txt"] = b"keep remote"
    with client(server) as transfer:
        with pytest.raises(cli.TransferError, match="already exists"):
            transfer.upload(source, "")
        with pytest.raises(cli.TransferError, match="already exists"):
            transfer.download("file.txt", target)
    assert target.read_bytes() == b"keep"
    assert server.files["file.txt"] == b"keep remote"
    # Covers a collision created after the initial destination existence check.
    with pytest.raises(FileExistsError):
        cli.publish_new(source, target)
    assert target.read_bytes() == b"keep" and source.read_bytes() == b"source"


@pytest.mark.parametrize(
    "name",
    ["../escape", "C:/file", "x\\file", "CON", "x:ads", ".WMCP-private", "LPT¹.txt", "x.", "a//b"],
)
def test_unsafe_remote_paths_are_rejected(name):
    with pytest.raises(cli.TransferError):
        cli.remote_path(name)


def test_malicious_remote_listing_cannot_escape_destination(tmp_path):
    def handle(request):
        return httpx.Response(
            200,
            json={
                "kind": "Index",
                "dir_exists": True,
                "paths": [{"name": "../escape", "path_type": "File", "size": 3}],
            },
        )

    with cli.Client(
        "http://127.0.0.1:8002",
        TOKEN,
        cli.Reporter(json_lines=True),
        transport=httpx.MockTransport(handle),
    ) as transfer:
        with pytest.raises(cli.TransferError):
            transfer.download("", tmp_path / "target")
    assert not (tmp_path / "target").exists()
    assert not (tmp_path / "escape").exists()


def test_redirects_are_not_followed_and_token_is_not_forwarded():
    requests = []

    def handle(request):
        requests.append(str(request.url))
        return httpx.Response(302, headers={"location": "https://untrusted.example/"})

    with cli.Client(
        "http://127.0.0.1:8002",
        TOKEN,
        cli.Reporter(json_lines=True),
        transport=httpx.MockTransport(handle),
    ) as transfer:
        with pytest.raises(cli.TransferError):
            transfer.connect()
    assert requests == ["http://127.0.0.1:8002/v1/info"]


def test_main_redacts_errors_and_accepts_common_options_after_action(monkeypatch, capsys):
    monkeypatch.setenv("WINDOWS_MCP_TRANSFER_TOKEN", TOKEN)
    monkeypatch.setattr(cli.Client, "connect", lambda self: None)

    def fail(self, path):
        raise cli.TransferError("transfer_network", f"secret: {TOKEN}")

    monkeypatch.setattr(cli.Client, "list", fail)
    assert cli.main(["list", "--json", "--url", "http://127.0.0.1:8002"]) == 1
    captured = capsys.readouterr()
    assert TOKEN not in captured.out + captured.err
    result = json.loads(captured.out)
    assert result["event"] == "error" and "[REDACTED]" in result["message"]


def test_local_symlinks_are_not_followed(tmp_path):
    source = tmp_path / "source"
    source.mkdir()
    outside = tmp_path / "outside"
    outside.write_bytes(b"private")
    try:
        (source / "link").symlink_to(outside)
    except OSError:
        pytest.skip("Symlink creation is unavailable for this Windows user")
    with pytest.raises(cli.TransferError):
        cli.local_plan(source)
