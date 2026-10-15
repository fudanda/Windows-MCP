"""Verify display inventory and screenshots against the running desktop manager backend."""

import argparse
import asyncio
import base64
import io
import json
import os
import time
from pathlib import Path
from typing import Any

import httpx2
import win32crypt
from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client
from mcp.types import CallToolResult
from PIL import Image

DEFAULT_MAX_SSE_EVENT_SIZE = 1024 * 1024 * 1024


def summarize(result: CallToolResult, output: Path, prefix: str) -> dict[str, Any]:
    """Save returned image content without printing its base64 payload."""
    summary: dict[str, Any] = {"isError": bool(result.is_error), "text": [], "images": []}
    if result.structured_content is not None:
        summary["structuredContent"] = result.structured_content
    for content in result.content:
        if content.type == "text":
            summary["text"].append(content.text)
        elif content.type == "image":
            data = base64.b64decode(content.data, validate=True)
            extension = "png" if content.mime_type == "image/png" else "jpg"
            path = output / f"{prefix}-{len(summary['images']) + 1}.{extension}"
            with Image.open(io.BytesIO(data)) as image:
                dimensions = list(image.size)
                extrema = image.convert("RGB").getextrema()
            path.write_bytes(data)
            summary["images"].append(
                {
                    "file": str(path),
                    "mimeType": content.mime_type,
                    "bytes": len(data),
                    "dimensions": dimensions,
                    "nonUniformPixels": any(lo != hi for lo, hi in extrema),
                }
            )
    return summary


async def verify(config_path: Path, output: Path, max_sse_event_size: int) -> bool:
    """Call the real read-only MCP tools and write a reproducible verification report."""
    stored = json.loads(config_path.read_text(encoding="utf-8"))
    token = win32crypt.CryptUnprotectData(bytes(stored["protectedToken"]), None, None, None, 0)[
        1
    ].decode()
    output.mkdir(parents=True, exist_ok=True)
    url = f"http://127.0.0.1:{stored['port']}/mcp"
    report: dict[str, Any] = {
        "endpoint": url,
        "maxSseEventSizeBytes": max_sse_event_size,
        "calls": {},
    }
    async with httpx2.AsyncClient(
        headers={"Authorization": f"Bearer {token}"},
        trust_env=False,
        timeout=60,
    ) as http:
        async with streamable_http_client(
            url,
            http_client=http,
            max_sse_event_size=max_sse_event_size,
        ) as (read, write):
            async with ClientSession(read, write) as session:
                initialized = await session.initialize()
                report["serverInfo"] = initialized.server_info.model_dump(mode="json")
                listed = await session.list_tools()
                names = {tool.name for tool in listed.tools}
                report["toolCount"] = len(names)
                report["requiredToolsFound"] = {"Screenshot", "DisplayInventory"} <= names
                if not report["requiredToolsFound"]:
                    raise RuntimeError("Required capture tools are unavailable")
                for name, arguments in [
                    ("DisplayInventory", {}),
                    ("Screenshot", {"use_annotation": False}),
                ]:
                    started = time.monotonic()
                    result = await session.call_tool(name, arguments)
                    for _ in range(4):
                        text = " ".join(c.text for c in result.content if c.type == "text")
                        if not result.is_error or not any(
                            code in text for code in ["USER_CONTROL", "TAKEOVER_PENDING"]
                        ):
                            break
                        await asyncio.sleep(4)
                        result = await session.call_tool(name, arguments)
                    prefix = "official-mcp-desktop" if name == "Screenshot" else "display"
                    summary = summarize(result, output, prefix)
                    summary["elapsedSeconds"] = round(time.monotonic() - started, 3)
                    report["calls"][name] = summary
    displays = report["calls"]["DisplayInventory"]
    screenshot = report["calls"]["Screenshot"]
    report["passed"] = (
        not displays["isError"]
        and not screenshot["isError"]
        and bool(displays.get("structuredContent", {}).get("result"))
        and bool(screenshot["images"])
        and all(image["nonUniformPixels"] for image in screenshot["images"])
    )
    report_path = output / "official-mcp-capture-test.json"
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(
        json.dumps(
            {
                "passed": report["passed"],
                "maxSseEventSizeBytes": max_sse_event_size,
                "toolCount": report["toolCount"],
                "images": screenshot["images"],
                "report": str(report_path),
            },
            ensure_ascii=False,
        )
    )
    return report["passed"]


def main() -> None:
    """Parse optional overrides; default each incoming SSE event to at most 1 GiB."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--config",
        type=Path,
        default=Path(os.environ["APPDATA"]) / "com.fudanda.windowsmcp.manager/manager.json",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "test-results",
    )
    parser.add_argument(
        "--max-sse-event-size",
        type=int,
        default=DEFAULT_MAX_SSE_EVENT_SIZE,
        help="Maximum bytes per incoming SSE event (default: 1073741824 / 1 GiB)",
    )
    args = parser.parse_args()
    if args.max_sse_event_size <= 0:
        parser.error("--max-sse-event-size must be greater than zero")
    if not asyncio.run(verify(args.config, args.output_dir.resolve(), args.max_sse_event_size)):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
