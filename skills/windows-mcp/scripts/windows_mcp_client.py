# /// script
# requires-python = ">=3.10"
# dependencies = ["mcp==2.3.0", "httpx2==2.13.1"]
# ///
"""Portable MCP calls and verified bootstrap of the existing transfer client."""

from __future__ import annotations

import argparse
import asyncio
import base64
import getpass
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from typing import Any
from urllib.parse import urlsplit
import uuid

import httpx2
from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client
from mcp.types import PaginatedRequestParams

MAX_CLIENT_BYTES = 1024 * 1024
LAN_NETWORKS = tuple(
    ipaddress.ip_network(network)
    for network in (
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "127.0.0.0/8",
        "169.254.0.0/16",
        "::1/128",
        "fc00::/7",
        "fe80::/10",
    )
)


def mcp_url(value: str) -> str:
    """Accept an explicit endpoint without embedded credentials or query secrets."""
    parsed = urlsplit(value)
    if (
        parsed.scheme not in {"http", "https"}
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.query
        or parsed.fragment
        or parsed.port == 0
    ):
        raise ValueError("Set WINDOWS_MCP_URL to the explicit HTTP MCP endpoint")
    return value.rstrip("/")


def transfer_origin(endpoint: str, port: Any) -> str:
    """Use the selected MCP host, never the remote device's loopback advertisement."""
    host = urlsplit(endpoint).hostname or ""
    address = ipaddress.ip_address("127.0.0.1" if host == "localhost" else host)
    if not any(address in network for network in LAN_NETWORKS):
        raise ValueError("Transfer bootstrap requires a loopback or LAN IP address")
    if isinstance(port, bool) or not isinstance(port, int) or not 1 <= port <= 65535:
        raise ValueError("Invalid file-service port")
    return f"http://{'[' + host + ']' if address.version == 6 else host}:{port}"


def remember_tokens(value: Any, secrets: set[str]) -> None:
    """Collect returned credentials before serializing structured or text content."""
    if isinstance(value, dict):
        for key, item in value.items():
            if (
                str(key).replace("_", "").casefold()
                in {
                    "token",
                    "accesstoken",
                    "refreshtoken",
                    "authorization",
                }
                and isinstance(item, str)
                and item
                and item != "[REDACTED]"
            ):
                secrets.add(item)
                if item.startswith("Bearer "):
                    secrets.add(item[7:])
            remember_tokens(item, secrets)
    elif isinstance(value, list):
        for item in value:
            remember_tokens(item, secrets)
    elif isinstance(value, str) and value.lstrip().startswith(("{", "[")):
        try:
            remember_tokens(json.loads(value), secrets)
        except ValueError:
            pass


def redact(value: Any, secrets: set[str]) -> str:
    """Serialize JSON without exposing credentials known to this process."""
    remember_tokens(value, secrets)

    def clean(item: Any) -> Any:
        if isinstance(item, dict):
            return {clean(str(key)): clean(part) for key, part in item.items()}
        if isinstance(item, list):
            return [clean(part) for part in item]
        if isinstance(item, str):
            if item.lstrip().startswith(("{", "[")):
                try:
                    return json.dumps(clean(json.loads(item)), ensure_ascii=False)
                except ValueError:
                    pass
            for secret in sorted(secrets, key=len, reverse=True):
                item = item.replace(secret, "[REDACTED]")
        return item

    return json.dumps(clean(value), ensure_ascii=False)


def unpack(result: Any) -> dict[str, Any]:
    """Read a successful structured MCP object, including SDK result wrappers."""
    if result.is_error:
        raise RuntimeError(" ".join(item.text for item in result.content if item.type == "text"))
    value = result.structured_content
    if value is None:
        value = json.loads(next(item.text for item in result.content if item.type == "text"))
    if isinstance(value, dict) and set(value) == {"result"} and isinstance(value["result"], dict):
        value = value["result"]
    if not isinstance(value, dict):
        raise ValueError("Expected a structured MCP object")
    return value


def error_detail(error: BaseException) -> str:
    """Include actionable transport errors wrapped by async SDK task groups."""
    nested = getattr(error, "exceptions", None)
    if nested:
        return "; ".join(error_detail(item) for item in nested)
    return f"{type(error).__name__}: {error}"


def summarize(result: Any, directory: Path) -> dict[str, Any]:
    """Save image blocks as new files and keep stdout suitable for agents."""
    output = {
        "isError": bool(result.is_error),
        "structuredContent": result.structured_content,
        "text": [],
        "images": [],
    }
    for item in result.content:
        if item.type == "text":
            output["text"].append(item.text)
        elif item.type == "image":
            extension = {"image/png": ".png", "image/jpeg": ".jpg", "image/webp": ".webp"}.get(
                item.mime_type
            )
            if not extension:
                raise ValueError(f"Unsupported image MIME type: {item.mime_type}")
            content = base64.b64decode(item.data, validate=True)
            directory.mkdir(parents=True, exist_ok=True)
            path = directory.resolve() / (uuid.uuid4().hex + extension)
            with path.open("xb") as stream:
                stream.write(content)
            output["images"].append(
                {"path": str(path), "mimeType": item.mime_type, "bytes": len(content)}
            )
    return output


async def download_client(http: httpx2.AsyncClient, origin: str, metadata: dict) -> bytes:
    """Bound and hash the advertised script before any execution."""
    if metadata.get("path") != "/client/windows-mcp-transfer.py":
        raise ValueError("Unexpected transfer client path")
    expected = metadata.get("sha256", "")
    if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", expected):
        raise ValueError("Invalid transfer client SHA-256")
    content = bytearray()
    async with http.stream("GET", origin + metadata["path"]) as response:
        response.raise_for_status()
        async for chunk in response.aiter_bytes():
            content.extend(chunk)
            if len(content) > MAX_CLIENT_BYTES:
                raise ValueError("Transfer client exceeds the 1 MiB bootstrap limit")
    if hashlib.sha256(content).hexdigest() != expected.lower():
        raise ValueError("Transfer client SHA-256 mismatch; script was not executed")
    return bytes(content)


async def transfer(
    session: ClientSession, endpoint: str, arguments: list[str], timeout: float, secrets: set[str]
) -> int:
    """Delegate transfers to the served CLI using child-process-only credentials."""
    if arguments[:1] == ["--"]:
        arguments = arguments[1:]
    if not arguments:
        raise ValueError("Use transfer -- [--json] upload/download/list/status/cancel ...")
    blocked = {"--url", "--token", "--token-file", "--token-stdin"}
    # argparse accepts unambiguous option prefixes in the served CLI. Block those
    # too, so --ur or --token-f cannot override the selected connection.
    if any(
        argument.startswith("--")
        and argument != "--"
        and any(option.startswith(argument.split("=", 1)[0]) for option in blocked)
        for argument in arguments
    ):
        raise ValueError("Transfer URL and credentials come from the selected MCP connection")
    status = unpack(await session.call_tool("FileTransfer", {"action": "status"}))
    if not status.get("running"):
        raise ValueError('File service is stopped; call FileTransfer with {"action":"start"}')
    connection = unpack(await session.call_tool("FileTransfer", {"action": "connection"}))
    token = connection.get("token")
    if not isinstance(token, str) or not token:
        raise ValueError("File service did not return a transfer token")
    secrets.add(token)
    origin = transfer_origin(endpoint, connection.get("port"))
    # No credential is needed for the public client script. Never follow redirects.
    async with httpx2.AsyncClient(trust_env=False, follow_redirects=False, timeout=timeout) as http:
        content = await download_client(http, origin, connection["client"])
    with tempfile.TemporaryDirectory(prefix="windows-mcp-client-") as temporary:
        script = Path(temporary) / "windows-mcp-transfer.py"
        script.write_bytes(content)
        env = os.environ.copy()
        env.pop("WINDOWS_MCP_TOKEN", None)
        env["WINDOWS_MCP_TRANSFER_URL"] = origin
        env["WINDOWS_MCP_TRANSFER_TOKEN"] = token
        command = [
            "uv",
            "run",
            "--no-project",
            "--python",
            sys.executable,
            "--script",
            str(script),
            *arguments,
        ]
        # Inherit the caller's working directory and terminal: relative project paths
        # retain their meaning, and Rich can render its normal progress bar.
        return await asyncio.to_thread(subprocess.call, command, env=env)


async def run(args: argparse.Namespace, token: str, secrets: set[str]) -> int:
    """Open one authenticated SDK session for the requested action."""
    endpoint = mcp_url(args.url)
    async with httpx2.AsyncClient(
        headers={"Authorization": f"Bearer {token}"},
        trust_env=False,
        follow_redirects=False,
        timeout=args.timeout,
    ) as http:
        async with streamable_http_client(
            endpoint, http_client=http, max_sse_event_size=args.max_sse_event_size
        ) as (read, write):
            async with ClientSession(read, write) as session:
                initialized = await session.initialize()
                if args.command == "tools":
                    tools = []
                    cursor = None
                    while True:
                        page = await session.list_tools(
                            params=PaginatedRequestParams(cursor=cursor) if cursor else None
                        )
                        tools.extend(
                            {
                                "name": tool.name,
                                "description": tool.description,
                                "inputSchema": tool.input_schema,
                            }
                            for tool in page.tools
                        )
                        cursor = page.next_cursor
                        if not cursor:
                            break
                    print(
                        redact(
                            {
                                "serverInfo": initialized.server_info.model_dump(mode="json"),
                                "tools": tools,
                            },
                            secrets,
                        ),
                        flush=True,
                    )
                    return 0
                if args.command == "transfer":
                    return await transfer(session, endpoint, args.arguments, args.timeout, secrets)
                arguments = json.loads(
                    args.args_file.read_text(encoding="utf-8-sig")
                    if args.args_file
                    else args.args_json
                )
                if not isinstance(arguments, dict):
                    raise ValueError("Tool arguments must be a JSON object")
                remember_tokens(arguments, secrets)
                result = await session.call_tool(args.tool, arguments)
                output = summarize(result, args.output_dir)
                print(redact({"tool": args.tool, **output}, secrets), flush=True)
                return 1 if result.is_error else 0


def main() -> int:
    """Parse portable CLI options without accepting tokens in process arguments."""
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description="Windows-MCP 桌面工具和文件传输助手")
    parser.add_argument("--url", default=os.environ.get("WINDOWS_MCP_URL", ""))
    parser.add_argument("--token-stdin", action="store_true")
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--max-sse-event-size", type=int, default=1024**3)
    parser.add_argument("--output-dir", type=Path, default=Path("artifacts/windows-mcp"))
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("tools", help="发现工具及参数")
    call = commands.add_parser("call", help="调用工具，图片保存为本机文件")
    call.add_argument("tool")
    inputs = call.add_mutually_exclusive_group()
    inputs.add_argument("--args-json", default="{}")
    inputs.add_argument("--args-file", type=Path)
    commands.add_parser("transfer", help="核对 SHA-256 后运行现有传输客户端").add_argument(
        "arguments", nargs=argparse.REMAINDER
    )
    args = parser.parse_args()
    secrets: set[str] = set()
    try:
        if not args.url:
            raise ValueError("Set WINDOWS_MCP_URL or --url")
        if args.timeout <= 0 or args.max_sse_event_size <= 0:
            raise ValueError("Timeout and SSE limit must be positive")
        token = (
            sys.stdin.readline(1025).strip()
            if args.token_stdin
            else os.environ.get("WINDOWS_MCP_TOKEN", "")
        )
        if not token and sys.stdin.isatty() and not args.token_stdin:
            token = getpass.getpass("MCP Token: ").strip()
        if not token or any(ord(char) < 33 or ord(char) > 126 for char in token):
            raise ValueError(
                "Set WINDOWS_MCP_TOKEN or use --token-stdin (raw token, no Bearer prefix)"
            )
        secrets.add(token)
        return asyncio.run(run(args, token, secrets))
    except KeyboardInterrupt:
        return 130
    except Exception as error:
        print(
            redact(
                {"ok": False, "error": type(error).__name__, "detail": error_detail(error)}, secrets
            ),
            flush=True,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
