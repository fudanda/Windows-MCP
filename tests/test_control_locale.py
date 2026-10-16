"""Language persistence and runtime changes of the desktop-control notice."""

import json
from pathlib import Path
from unittest.mock import Mock

import pytest

from windows_mcp.desktop import control_locale


@pytest.fixture(autouse=True)
def _isolated_environment(monkeypatch):
    monkeypatch.delenv("WINDOWS_MCP_LANGUAGE", raising=False)
    monkeypatch.delenv("WINDOWS_MCP_MANAGER_CONFIG", raising=False)


@pytest.mark.parametrize(
    "value,expected", [(None, "zh-CN"), ("zh-CN", "zh-CN"), ("en", "en"), ("unknown", "zh-CN")]
)
def test_standalone_language_defaults_to_simplified_chinese(monkeypatch, value, expected):
    if value is not None:
        monkeypatch.setenv("WINDOWS_MCP_LANGUAGE", value)
    assert control_locale.NoticeLanguage().current() == expected


def test_runtime_switch_uses_saved_language_and_keeps_last_valid_value(monkeypatch, tmp_path):
    config = tmp_path / "manager.json"
    monkeypatch.setenv("WINDOWS_MCP_MANAGER_CONFIG", str(config))
    clock = Mock(return_value=1.0)
    monkeypatch.setattr(control_locale.time, "monotonic", clock)
    language = control_locale.NoticeLanguage()
    assert language.current() == "zh-CN"  # Missing file is harmless.
    for tick, content, expected in [
        (2, '{"language":"en"}', "en"),
        (3, '{"language":', "en"),
        (4, '{"language":"not-supported"}', "en"),
        (5, "[]", "en"),
        (6, '{"language":"zh-CN"}', "zh-CN"),
    ]:
        config.write_text(content, encoding="utf-8")
        clock.return_value = tick
        assert language.current() == expected
    config.unlink()
    clock.return_value = 7
    assert language.current() == "zh-CN"


def test_configuration_wins_over_startup_environment_and_polling_is_bounded(monkeypatch, tmp_path):
    config = tmp_path / "manager.json"
    config.write_text(json.dumps({"language": "zh-CN"}), encoding="utf-8")
    monkeypatch.setenv("WINDOWS_MCP_LANGUAGE", "en")
    monkeypatch.setenv("WINDOWS_MCP_MANAGER_CONFIG", str(config))
    clock = Mock(return_value=1.0)
    monkeypatch.setattr(control_locale.time, "monotonic", clock)
    language = control_locale.NoticeLanguage()
    assert language.current() == "zh-CN"
    config.write_text(json.dumps({"language": "en"}), encoding="utf-8")
    clock.return_value = 1.1
    assert language.current() == "zh-CN"
    clock.return_value = 1.5
    assert language.current() == "en"


def test_temporarily_unreadable_configuration_does_not_break_notice(monkeypatch, tmp_path):
    monkeypatch.setenv("WINDOWS_MCP_LANGUAGE", "en")
    monkeypatch.setenv("WINDOWS_MCP_MANAGER_CONFIG", str(tmp_path / "manager.json"))
    monkeypatch.setattr(Path, "read_text", Mock(side_effect=PermissionError("locked")))
    assert control_locale.NoticeLanguage().current() == "en"
