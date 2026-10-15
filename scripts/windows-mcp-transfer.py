# /// script
# requires-python = ">=3.10"
# dependencies = ["httpx==0.28.1", "rich==14.3.4"]
# ///
"""Standalone LAN transfer CLI. Run with uv; no Windows-MCP desktop client is required."""

from __future__ import annotations

import argparse
import hashlib
import ipaddress
import itertools
import json
import os
import re
import stat
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterator
from urllib.parse import quote, urlsplit

try:
    import httpx
    from rich.console import Console
    from rich.progress import (
        BarColumn,
        DownloadColumn,
        Progress,
        TaskProgressColumn,
        TextColumn,
        TransferSpeedColumn,
    )
except ImportError:
    raise SystemExit("请使用 / Run: uv run --script windows-mcp-transfer.py --help") from None

VERSION = "0.2.1"
CHUNK = 256 * 1024
MAX_ENTRIES = 10_000
MAX_BYTES = 1024**4
MAX_METADATA = 8 * 1024 * 1024
TEXT = {
    "zh-CN": {
        "description": "Codex 局域网文件传输：无需浏览器或本机桌面客户端。",
        "url": "文件服务地址，也可设置 WINDOWS_MCP_TRANSFER_URL",
        "token_file": "从 UTF-8 文件读取传输 Token；默认读取 WINDOWS_MCP_TRANSFER_TOKEN",
        "token_stdin": "从标准输入读取 Token，避免出现在命令行参数中",
        "language": "界面语言，默认简体中文",
        "json": "输出 JSON Lines 进度和结果，供 Codex/脚本读取",
        "timeout": "网络空闲超时秒数，默认 60",
        "upload": "上传本机文件或文件夹，保留名称、目录结构和空目录",
        "download": "下载远程文件或文件夹，每个文件均校验 SHA-256",
        "list": "列出远程目录",
        "status": "查询服务端传输任务",
        "cancel": "取消服务端任务",
        "source": "本机文件或文件夹路径",
        "remote": "相对于共享目录的远程路径，使用 / 分隔",
        "upload_to": "远程父目录，默认共享目录；会保留源文件/文件夹名",
        "download_to": "本机最终目标路径，必须尚不存在",
        "hashing": "计算 SHA-256",
        "uploading": "上传",
        "downloading": "下载",
        "verifying": "校验",
        "verified": "SHA-256 校验通过",
        "completed": "完成",
        "cancelled": "已取消，未完成的文件不会发布",
        "error": "操作失败",
    },
    "en": {
        "description": "LAN file transfers for Codex, without a browser or local desktop client.",
        "url": "File service URL, or WINDOWS_MCP_TRANSFER_URL",
        "token_file": "Read the token from UTF-8 file; otherwise WINDOWS_MCP_TRANSFER_TOKEN",
        "token_stdin": "Read token from stdin, keeping it out of command-line arguments",
        "language": "Interface language (default: Simplified Chinese)",
        "json": "Emit JSON Lines progress and results for agents/scripts",
        "timeout": "Network idle timeout in seconds (default: 60)",
        "upload": "Upload a local file/folder, preserving names and empty directories",
        "download": "Download a remote file/folder with per-file SHA-256 verification",
        "list": "List a remote directory",
        "status": "Query server transfer tasks",
        "cancel": "Cancel a server task",
        "source": "Local file or directory path",
        "remote": "Remote path relative to the shared folder; use / separators",
        "upload_to": "Remote parent directory; source name is preserved (default: shared root)",
        "download_to": "Exact local destination path; must not exist",
        "hashing": "SHA-256",
        "uploading": "Upload",
        "downloading": "Download",
        "verifying": "Verify",
        "verified": "SHA-256 verified",
        "completed": "Completed",
        "cancelled": "Cancelled; incomplete files were not published",
        "error": "Operation failed",
    },
}


class TransferError(Exception):
    """A bounded, machine-readable transfer failure."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(detail)
        self.code = code


def component(name: str) -> str:
    """Validate one name using the receiving Windows filesystem's restrictions."""
    stem = name.split(".")[0].upper()
    if (
        not name
        or len(name.encode("utf-8")) > 240
        or name in {".", ".."}
        or name.endswith((".", " "))
        or name.lower().startswith(".wmcp-")
        or any(ord(c) < 32 or ord(c) == 127 or c in '<>:"/\\|?*' for c in name)
        or stem in {"CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"}
        or re.fullmatch(r"(?:COM|LPT)[1-9¹²³]", stem)
    ):
        raise TransferError("transfer_path", f"Unsupported file name: {name!r}")
    return name


def remote_path(value: str) -> str:
    """Normalize only the shared-root marker; never normalize traversal components away."""
    if value in {"", "/", "."}:
        return ""
    value = value.strip("/")
    if len(value.encode("utf-8")) > 2048:
        raise TransferError("transfer_path", "Remote path exceeds 2048 bytes")
    for part in value.split("/"):
        component(part)
    return value


def join_remote(parent: str, name: str) -> str:
    return remote_path(f"{parent}/{name}" if parent else name)


def service_url(value: str) -> str:
    """Restrict credentials to an explicit loopback/LAN service without redirects or proxies."""
    try:
        parsed = urlsplit(value)
        host = parsed.hostname or ""
        ip = ipaddress.ip_address("127.0.0.1" if host == "localhost" else host)
        networks = (
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "127.0.0.0/8",
            "169.254.0.0/16",
            "::1/128",
            "fc00::/7",
            "fe80::/10",
        )
        local = any(ip in ipaddress.ip_network(net) for net in networks)
        valid = (
            local
            and parsed.scheme in {"http", "https"}
            and parsed.path in {"", "/"}
            and not parsed.username
            and not parsed.password
            and not parsed.query
            and not parsed.fragment
            and parsed.port != 0
        )
    except ValueError:
        valid = False
    if not valid:
        raise TransferError("transfer_url", "Use http://LAN-IP:8002 (file service, not /mcp)")
    return value.rstrip("/")


def ordinary(path: Path) -> os.stat_result:
    """Do not follow symlinks, junctions, reparse points, or special files."""
    metadata = path.lstat()
    if (
        stat.S_ISLNK(metadata.st_mode)
        or getattr(metadata, "st_file_attributes", 0) & 0x400
        or not (stat.S_ISREG(metadata.st_mode) or stat.S_ISDIR(metadata.st_mode))
    ):
        raise TransferError("transfer_path", f"Links and special files are not supported: {path}")
    return metadata


def local_path(value: str) -> Path:
    path = Path(os.path.abspath(os.path.expanduser(value)))
    # abspath keeps symlinks visible, whereas resolve() would silently follow them.
    for parent in reversed((path, *path.parents)):
        if os.path.lexists(parent):
            ordinary(parent)
    return path


@dataclass
class Entry:
    path: str
    directory: bool
    size: int = 0
    local: Path | None = None


def check_plan(entries: list[Entry]) -> int:
    total = sum(entry.size for entry in entries)
    check_limits(len(entries), total)
    return total


def check_limits(count: int, total: int) -> None:
    if count > MAX_ENTRIES or total > MAX_BYTES:
        raise TransferError("transfer_limit", "Limit: 10,000 entries and 1 TiB per transfer")


def local_plan(source: Path) -> tuple[bool, list[Entry]]:
    root = ordinary(source)
    component(source.name)
    directory = stat.S_ISDIR(root.st_mode)
    entries = []
    total = 0
    pending = [(source, "")]
    while pending:
        path, relative = pending.pop()
        metadata = ordinary(path)
        if relative:
            remote_path(relative)
        is_dir = stat.S_ISDIR(metadata.st_mode)
        if relative or not directory:
            entries.append(Entry(relative, is_dir, 0 if is_dir else metadata.st_size, path))
            total += entries[-1].size
            check_limits(len(entries), total)
        if is_dir:
            children = list(itertools.islice(path.iterdir(), MAX_ENTRIES + 1))
            check_limits(len(children), 0)
            children.sort(key=lambda item: item.name.casefold())
            names = set()
            for child in children:
                component(child.name)
                if child.name.casefold() in names:
                    raise TransferError(
                        "transfer_path", f"Case-insensitive name collision: {child}"
                    )
                names.add(child.name.casefold())
            pending.extend(
                (child, join_remote(relative, child.name)) for child in reversed(children)
            )
    return directory, entries


class Reporter:
    """Rich terminal progress, or bounded JSON Lines for Codex without a terminal."""

    def __init__(self, language: str = "zh-CN", json_lines: bool = False) -> None:
        self.text = TEXT[language]
        self.json_lines = json_lines
        self.console = Console(stderr=True, markup=False, highlight=False)
        self.progress: Progress | None = None
        self.task: Any = None
        self.total = self.transferred = 0
        self.last = 0.0

    def __enter__(self) -> Reporter:
        if not self.json_lines and self.console.is_terminal:
            self.progress = Progress(
                TextColumn("{task.description}", markup=False),
                BarColumn(),
                TaskProgressColumn(),
                DownloadColumn(),
                TransferSpeedColumn(),
                console=self.console,
                redirect_stdout=False,
                redirect_stderr=False,
            )
            self.progress.start()
        return self

    def __exit__(self, *_: Any) -> None:
        if self.progress:
            self.progress.stop()

    def event(self, event: str, **data: Any) -> None:
        if self.json_lines:
            print(json.dumps({"event": event, **data}, ensure_ascii=False), flush=True)
        elif event in {"verified", "completed", "error", "cancelled"}:
            self.console.print(f"{self.text[event]}: {data.get('path', data.get('message', ''))}")

    def begin(self, total: int, files: int, directories: int) -> None:
        self.total = total
        self.event("plan", totalBytes=total, files=files, directories=directories)
        if self.progress:
            self.task = self.progress.add_task(self.text["hashing"], total=total)

    def update(self, phase: str, path: str, advanced: int = 0, force: bool = False) -> None:
        self.transferred += advanced
        if self.progress and self.task is not None:
            self.progress.update(
                self.task,
                completed=self.transferred,
                description=f"{self.text[phase]} {path}",
            )
        current = time.monotonic()
        if force or current - self.last >= 0.5:
            self.last = current
            data = {
                "phase": phase,
                "path": path,
                "transferredBytes": self.transferred,
                "totalBytes": self.total,
            }
            if self.json_lines:
                self.event("progress", **data)
            elif not self.progress:
                self.console.print(
                    f"{self.text[phase]} {path}: {self.transferred:,} / {self.total:,} B"
                )


class Client:
    """Use the existing authenticated gateway, never send file bytes through MCP."""

    def __init__(
        self,
        url: str,
        token: str,
        reporter: Reporter,
        timeout: float = 60,
        transport: httpx.BaseTransport | None = None,
    ) -> None:
        self.url = service_url(url)
        self.reporter = reporter
        self.http = httpx.Client(
            base_url=self.url,
            headers={"Authorization": f"Bearer {token}", "Accept-Encoding": "identity"},
            follow_redirects=False,
            trust_env=False,
            timeout=httpx.Timeout(timeout, connect=10),
            transport=transport,
        )

    def __enter__(self) -> Client:
        return self

    def __exit__(self, *_: Any) -> None:
        self.http.close()

    def file_url(self, path: str, query: str = "") -> str:
        return "/files/" + "/".join(quote(p, safe="") for p in remote_path(path).split("/")) + query

    def checked(self, response: httpx.Response) -> httpx.Response:
        if not response.is_success:
            code = {
                401: "transfer_auth",
                403: "transfer_conflict",
                409: "transfer_conflict",
                422: "transfer_integrity",
                404: "transfer_missing",
            }.get(response.status_code, "transfer_network")
            raise TransferError(code, f"HTTP {response.status_code}: {response.request.url.path}")
        return response

    def metadata(self, path: str) -> Any:
        with self.http.stream("GET", path) as response:
            self.checked(response)
            content = bytearray()
            for chunk in response.iter_bytes(CHUNK):
                content.extend(chunk)
                if len(content) > MAX_METADATA:
                    raise TransferError("transfer_limit", "Metadata exceeds 8 MiB")
        try:
            return json.loads(content)
        except (ValueError, UnicodeError) as exc:
            raise TransferError("transfer_protocol", "Invalid JSON metadata") from exc

    def connect(self) -> None:
        info = self.metadata("/v1/info")
        if not isinstance(info, dict) or info.get("protocol") != "windows-mcp-transfer/1":
            raise TransferError("transfer_protocol", "Not a Windows-MCP file service")

    def list(self, path: str) -> list[dict[str, Any]]:
        data = self.metadata(self.file_url(path, "?json"))
        if not isinstance(data, dict) or data.get("kind") != "Index" or not data.get("dir_exists"):
            raise TransferError("transfer_protocol", "Expected a directory listing")
        paths = data.get("paths")
        if not isinstance(paths, list) or len(paths) > MAX_ENTRIES:
            raise TransferError("transfer_limit", "Too many directory entries")
        names = set()
        for entry in paths:
            if not isinstance(entry, dict) or not isinstance(entry.get("name"), str):
                raise TransferError("transfer_protocol", "Invalid directory entry")
            name = component(entry["name"])
            if name.casefold() in names or entry.get("path_type") not in {"File", "Dir"}:
                raise TransferError("transfer_path", "Duplicate names or links in remote directory")
            names.add(name.casefold())
            if type(entry.get("size")) is not int or not 0 <= entry["size"] <= MAX_BYTES:
                raise TransferError("transfer_protocol", "Invalid file size")
        return paths

    def remote_plan(self, path: str) -> tuple[bool, list[Entry]]:
        path = remote_path(path)
        if path:
            parent, _, name = path.rpartition("/")
            matches = [e for e in self.list(parent) if e["name"].casefold() == name.casefold()]
            if not matches:
                raise TransferError("transfer_missing", f"Remote path not found: {path}")
            root = matches[0]
            if root["path_type"] == "File":
                return False, [Entry("", False, root["size"])]
        entries = []
        total = 0
        pending = [""]
        while pending:
            parent = pending.pop()
            for item in sorted(self.list(join_remote(path, parent)), key=lambda e: e["name"]):
                relative = join_remote(parent, item["name"])
                directory = item["path_type"] == "Dir"
                entries.append(Entry(relative, directory, 0 if directory else item["size"]))
                total += entries[-1].size
                check_limits(len(entries), total)
                if directory:
                    pending.append(relative)
        return True, entries

    def require_missing(self, path: str) -> None:
        response = self.http.head(self.file_url(path))
        if response.status_code == 404:
            return
        self.checked(response)
        raise TransferError("transfer_conflict", f"Destination already exists: {path}")

    def mkdir(self, path: str) -> None:
        self.checked(self.http.request("MKCOL", self.file_url(path)))

    def parents(self, path: str) -> None:
        current = ""
        for name in path.split("/") if path else []:
            listing = self.list(current)
            found = next((e for e in listing if e["name"].casefold() == name.casefold()), None)
            current = join_remote(current, name)
            if found is None:
                self.mkdir(current)
            elif found["path_type"] != "Dir":
                raise TransferError("transfer_conflict", f"Parent is a file: {current}")

    def upload(self, source: Path, parent: str) -> dict[str, Any]:
        directory, entries = local_plan(source)
        target = join_remote(remote_path(parent), source.name)
        # A missing parent also implies a missing target, so create parents first.
        self.parents(remote_path(parent))
        self.require_missing(target)
        self.reporter.begin(
            check_plan(entries),
            sum(not e.directory for e in entries),
            sum(e.directory for e in entries) + int(directory),
        )
        if directory:
            self.mkdir(target)
        verified = []
        for entry in entries:
            remote = join_remote(target, entry.path) if entry.path else target
            if entry.directory:
                self.mkdir(remote)
                continue
            assert entry.local is not None
            ordinary(entry.local)
            self.reporter.update("hashing", remote, force=True)
            digest = hashlib.sha256()
            with entry.local.open("rb") as handle:
                for chunk in iter(lambda: handle.read(CHUNK), b""):
                    digest.update(chunk)
                    self.reporter.update("hashing", remote)
                if handle.tell() != entry.size:
                    raise TransferError("transfer_integrity", f"Source changed: {entry.local}")
                expected = digest.hexdigest()
                handle.seek(0)

                def content() -> Iterator[bytes]:
                    for chunk in iter(lambda: handle.read(CHUNK), b""):
                        self.reporter.update("uploading", remote, len(chunk))
                        yield chunk
                    self.reporter.update("verifying", remote, force=True)

                response = self.checked(
                    self.http.put(
                        self.file_url(remote),
                        content=content(),
                        headers={"Content-Length": str(entry.size), "X-Content-SHA256": expected},
                    )
                )
            confirmation = response.json()
            if (
                not isinstance(confirmation, dict)
                or confirmation.get("verified") is not True
                or confirmation.get("sha256") != expected
            ):
                raise TransferError("transfer_integrity", f"Server did not verify {remote}")
            verified.append({"path": remote, "sha256": expected, "bytes": entry.size})
            self.reporter.event("verified", **verified[-1], taskId=confirmation.get("taskId"))
        return {
            "action": "upload",
            "source": str(source),
            "destination": target,
            "files": len(verified),
            "bytes": sum(e.size for e in entries),
            "sha256Verified": True,
            "checksums": verified,
        }

    def download_file(self, remote: str, target: Path, size: int) -> dict[str, Any]:
        require_local_missing(target)
        self.reporter.update("hashing", remote, force=True)
        with self.http.stream("GET", self.file_url(remote, "?hash")) as response:
            self.checked(response)
            content = b""
            for chunk in response.iter_bytes(128):
                content += chunk
                if len(content) > 128:
                    raise TransferError("transfer_protocol", "Invalid SHA-256 response")
        expected = content.decode("ascii").strip()
        if not re.fullmatch(r"[0-9a-f]{64}", expected):
            raise TransferError("transfer_integrity", "Remote SHA-256 is missing or invalid")
        digest, received = hashlib.sha256(), 0
        # Private same-filesystem staging is removed on checksum failure or Ctrl+C.
        with tempfile.TemporaryDirectory(prefix=".wmcp-cli-", dir=target.parent) as stage:
            payload = Path(stage) / "payload"
            with self.http.stream("GET", self.file_url(remote)) as response:
                self.checked(response)
                with payload.open("xb") as handle:
                    for chunk in response.iter_bytes(CHUNK):
                        received += len(chunk)
                        if received > size:
                            raise TransferError(
                                "transfer_integrity", "Download exceeds expected size"
                            )
                        handle.write(chunk)
                        digest.update(chunk)
                        self.reporter.update("downloading", remote, len(chunk))
                    handle.flush()
                    os.fsync(handle.fileno())
            self.reporter.update("verifying", remote, force=True)
            if received != size or digest.hexdigest() != expected:
                raise TransferError("transfer_integrity", f"SHA-256 or size mismatch: {remote}")
            publish_new(payload, target)
        result = {"path": remote, "sha256": expected, "bytes": received}
        self.reporter.event("verified", **result)
        return result

    def download(self, remote: str, target: Path) -> dict[str, Any]:
        remote = remote_path(remote)
        require_local_missing(target)
        directory, entries = self.remote_plan(remote)
        self.reporter.begin(
            check_plan(entries),
            sum(not e.directory for e in entries),
            sum(e.directory for e in entries) + int(directory),
        )
        target.parent.mkdir(parents=True, exist_ok=True)
        local_path(str(target.parent))
        if directory:
            target.mkdir()  # Exclusive creation: an existing directory is never merged or replaced.
        verified = []
        for entry in entries:
            destination = target.joinpath(*entry.path.split("/")) if entry.path else target
            local_path(str(destination))
            if entry.directory:
                destination.mkdir()
            else:
                destination.parent.mkdir(parents=True, exist_ok=True)
                verified.append(
                    self.download_file(join_remote(remote, entry.path), destination, entry.size)
                )
        return {
            "action": "download",
            "source": remote,
            "destination": str(target),
            "files": len(verified),
            "bytes": sum(e.size for e in entries),
            "sha256Verified": True,
            "checksums": verified,
        }


def require_local_missing(path: Path) -> None:
    if os.path.lexists(path):
        raise TransferError("transfer_conflict", f"Destination already exists: {path}")


def publish_new(source: Path, destination: Path) -> None:
    """Publish a verified file without a check-then-replace race."""
    local_path(str(destination.parent))
    if os.name == "nt":
        os.rename(source, destination)  # On Windows rename fails if destination exists.
    else:
        os.link(source, destination)  # POSIX rename would silently replace an existing file.
        source.unlink()


def parser(language: str) -> argparse.ArgumentParser:
    text = TEXT[language]
    common = argparse.ArgumentParser(add_help=False, argument_default=argparse.SUPPRESS)
    common.add_argument("--url", help=text["url"])
    credentials = common.add_mutually_exclusive_group()
    credentials.add_argument("--token-file", help=text["token_file"])
    credentials.add_argument("--token-stdin", action="store_true", help=text["token_stdin"])
    common.add_argument("--language", choices=TEXT, help=text["language"])
    common.add_argument("--json", action="store_true", help=text["json"])
    common.add_argument("--timeout", type=float, help=text["timeout"])
    result = argparse.ArgumentParser(description=text["description"], parents=[common])
    result.add_argument("--version", action="version", version=VERSION)
    commands = result.add_subparsers(dest="action", required=True)
    for action in ("upload", "download", "list", "status", "cancel"):
        command = commands.add_parser(
            action, help=text[action], description=text[action], parents=[common]
        )
        if action == "upload":
            command.add_argument("source", help=text["source"])
            command.add_argument("--to", default="", help=text["upload_to"])
        elif action == "download":
            command.add_argument("remote", help=text["remote"])
            command.add_argument("--to", required=True, help=text["download_to"])
        elif action == "list":
            command.add_argument("remote", nargs="?", default="", help=text["remote"])
        elif action == "cancel":
            command.add_argument("task_id")
    return result


def main(argv: list[str] | None = None) -> int:
    """Run a client-side transfer; stdout contains no file contents or credentials."""
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")
    pre = argparse.ArgumentParser(add_help=False)
    pre.add_argument("--language", choices=TEXT, default="zh-CN")
    language = pre.parse_known_args(argv)[0].language
    args = parser(language).parse_args(argv)
    token = ""
    reporter = Reporter(language, getattr(args, "json", False))
    try:
        if getattr(args, "token_stdin", False):
            token = sys.stdin.readline(1025).strip()
        elif getattr(args, "token_file", None):
            with open(args.token_file, encoding="utf-8-sig") as handle:
                token = handle.read(1025).strip()
        else:
            token = os.environ.get("WINDOWS_MCP_TRANSFER_TOKEN", "")
        if not 32 <= len(token) <= 256 or not all(33 <= ord(c) <= 126 for c in token):
            raise TransferError("transfer_auth", "Set WINDOWS_MCP_TRANSFER_TOKEN or --token-stdin")
        timeout = getattr(args, "timeout", 60)
        if not 1 <= timeout <= 3600:
            raise TransferError("transfer_timeout", "Idle timeout must be 1–3600 seconds")
        url = getattr(args, "url", os.environ.get("WINDOWS_MCP_TRANSFER_URL", ""))
        with reporter, Client(url, token, reporter, timeout) as client:
            client.connect()
            if args.action == "upload":
                result = client.upload(local_path(args.source), args.to)
            elif args.action == "download":
                result = client.download(args.remote, local_path(args.to))
            elif args.action == "list":
                result = {"path": remote_path(args.remote), "entries": client.list(args.remote)}
            elif args.action == "status":
                result = client.metadata("/v1/tasks")
            else:
                component(args.task_id)
                client.checked(client.http.delete("/v1/tasks/" + quote(args.task_id, safe="")))
                result = {"cancelled": True, "taskId": args.task_id}
        result = {"ok": True, **result}
        if reporter.json_lines:
            reporter.event("completed", **result)
        else:
            reporter.event("completed", path=result.get("destination", args.action))
            print(json.dumps(result, ensure_ascii=False), flush=True)
        return 0
    except KeyboardInterrupt:
        reporter.event(
            "cancelled", ok=False, code="transfer_cancelled", message=TEXT[language]["cancelled"]
        )
        return 130
    except (TransferError, httpx.HTTPError, OSError, ValueError) as exc:
        detail = str(exc).replace(token, "[REDACTED]") if token else str(exc)
        reporter.event(
            "error", ok=False, code=getattr(exc, "code", "transfer_network"), message=detail
        )
        if not reporter.json_lines:
            print(
                json.dumps(
                    {
                        "ok": False,
                        "code": getattr(exc, "code", "transfer_network"),
                        "message": detail,
                    },
                    ensure_ascii=False,
                ),
                flush=True,
            )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
