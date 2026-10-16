"""Prevent activating a different app when the requested window is missing or ambiguous."""

from unittest.mock import Mock
from dataclasses import replace

import pytest

from windows_mcp.desktop.service import Desktop


@pytest.fixture
def desktop(sample_window, sample_desktop_state):
    desktop = Desktop.__new__(Desktop)
    desktop.desktop_state = sample_desktop_state
    desktop.get_windows = Mock(return_value=([sample_window], {sample_window.handle}))
    desktop.bring_window_to_top = Mock()
    return desktop


def window(sample_window, title, handle):
    return replace(sample_window, name=title, handle=handle)


def test_resolves_new_window_absent_from_cached_snapshot(desktop, sample_window):
    manager = window(sample_window, "Windows-MCP 管理器", 1)
    fixture = window(sample_window, "Windows-MCP Input Test 20261016-0941", 2)
    desktop.desktop_state = replace(desktop.desktop_state, active_window=None, windows=[manager])
    desktop.get_windows.return_value = ([manager, fixture], {1, 2})

    assert desktop._find_window_by_name(fixture.name) == (fixture, "")
    desktop.get_windows.assert_called_once()


def test_missing_long_title_does_not_fuzzy_match_manager(desktop, sample_window):
    manager = window(sample_window, "Windows-MCP 管理器", 1)
    desktop.get_windows.return_value = ([manager], {1})

    response, status = desktop.switch_app("Windows-MCP Input Test 20261016-0941")

    assert status == 1
    assert "not found" in response
    desktop.bring_window_to_top.assert_not_called()


def test_exact_match_wins_over_other_partial_matches(desktop, sample_window):
    exact = window(sample_window, "Windows-MCP", 1)
    extended = window(sample_window, "Windows-MCP Manager", 2)
    desktop.get_windows.return_value = ([extended, exact], {1, 2})

    assert desktop._find_window_by_name("  WINDOWS-mcp  ") == (exact, "")


def test_unique_substring_preserves_localized_title(desktop, sample_window):
    fixture = window(sample_window, "中文测试 – Windows-MCP Input", 1)
    desktop.get_windows.return_value = ([sample_window, fixture], {1, sample_window.handle})

    assert desktop._find_window_by_name("中文测试") == (fixture, "")


@pytest.mark.parametrize("titles", [("Editor – first", "Editor – second"), ("Editor", "Editor")])
def test_ambiguous_titles_fail_without_activating(desktop, sample_window, titles):
    desktop.get_windows.return_value = (
        [window(sample_window, title, handle) for handle, title in enumerate(titles, start=1)],
        {1, 2},
    )

    response, status = desktop.switch_app("Editor")

    assert status == 1
    assert "Ambiguous" in response
    assert "HWND 1" in response and "HWND 2" in response
    desktop.bring_window_to_top.assert_not_called()


def test_closed_window_in_cached_snapshot_is_not_selected(desktop):
    desktop.get_windows.return_value = ([], set())

    assert desktop.switch_app("Notepad")[1] == 1
    desktop.bring_window_to_top.assert_not_called()


@pytest.mark.parametrize("title", [None, "", "  "])
def test_empty_title_does_not_select_a_window(desktop, title):
    assert desktop.switch_app(title)[1] == 1
    desktop.get_windows.assert_not_called()
    desktop.bring_window_to_top.assert_not_called()


def test_named_resize_rejects_missing_target(desktop, sample_window):
    desktop.get_windows.return_value = ([window(sample_window, "Windows-MCP 管理器", 1)], {1})
    response, status = desktop.resize_app("Windows-MCP Input Test", size=(500, 300))

    assert status == 1
    assert "not found" in response
