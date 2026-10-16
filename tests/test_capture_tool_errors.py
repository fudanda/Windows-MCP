"""Capture failures must be MCP errors instead of successful text-only results."""

from unittest.mock import Mock

import pytest
from fastmcp import Client, FastMCP

from windows_mcp.tools import snapshot


@pytest.mark.parametrize("name", ["Screenshot", "Snapshot"])
async def test_capture_failure_sets_mcp_is_error(name):
    desktop = Mock()
    desktop.get_state.side_effect = ValueError("region does not overlap the virtual screen")
    server = FastMCP("capture-error-regression")
    snapshot.register(server, get_desktop=lambda: desktop, get_analytics=lambda: None)

    async with Client(server) as client:
        result = await client.call_tool(
            name, {"region": [5000, 5000, 5100, 5100]}, raise_on_error=False
        )

    assert result.is_error is True
    assert "region does not overlap" in result.content[0].text
