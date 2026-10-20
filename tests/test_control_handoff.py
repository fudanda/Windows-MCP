"""A hardware reservation must never overlap an MCP input lease."""

import json
import time

import pytest

from windows_mcp.desktop import control
from windows_mcp.desktop.control_handoff import DesktopHandoff


def request(path, owner="robot-one", updated=None):
    path.mkdir(exist_ok=True)
    (path / "request.json").write_text(
        json.dumps(
            {
                "version": 1,
                "ownerId": owner,
                "updatedAt": updated or time.time() * 1000,
            }
        )
    )


def ready_owner(tmp_path):
    owner = control.ControlCoordinator()
    owner.handoff = DesktopHandoff(tmp_path)
    with owner._lock:
        owner._set_locked("ready")
    return owner


def test_idle_ai_lease_released_and_new_mcp_calls_blocked(tmp_path):
    owner = ready_owner(tmp_path)
    token = owner.begin_call("Screenshot")
    owner.end_call(token)
    request(tmp_path)
    snapshot = owner.publish_handoff()
    assert snapshot["state"] == "ready"
    assert snapshot["acknowledgedOwner"] == "robot-one"
    assert not owner._suppress
    with pytest.raises(control.ControlBlocked, match="HARDWARE_CONTROL"):
        owner.begin_call("Type")


def test_active_mcp_call_never_acknowledged(tmp_path):
    owner = ready_owner(tmp_path)
    token = owner.begin_call("Type")
    request(tmp_path)
    assert owner.publish_handoff()["acknowledgedOwner"] is None
    owner.checkpoint(token)
    owner.end_call(token)
    assert owner.publish_handoff()["acknowledgedOwner"] == "robot-one"


def test_expired_request_releases_hardware_reservation(tmp_path):
    owner = ready_owner(tmp_path)
    request(tmp_path)
    owner.publish_handoff()
    request(tmp_path, updated=time.time() * 1000 - 6000)
    assert owner.publish_handoff()["acknowledgedOwner"] is None
    owner.begin_call("Type")


def test_new_owner_cannot_inherit_previous_ack_during_user_control(tmp_path):
    owner = ready_owner(tmp_path)
    request(tmp_path)
    owner.publish_handoff()
    with owner._lock:
        owner._last_physical_event = time.monotonic()
        owner._set_locked("user")
    assert owner.publish_handoff()["acknowledgedOwner"] == "robot-one"
    request(tmp_path, owner="robot-two")
    assert owner.publish_handoff()["acknowledgedOwner"] is None


@pytest.mark.parametrize("payload", ["broken", "{}", '{"version":2}', "[1]", '"hello"'])
def test_malformed_request_never_grants_hardware_ownership(tmp_path, payload):
    owner = ready_owner(tmp_path)
    (tmp_path / "request.json").write_text(payload)
    assert owner.publish_handoff()["acknowledgedOwner"] is None


def test_unavailable_control_revokes_ack(tmp_path):
    owner = ready_owner(tmp_path)
    request(tmp_path)
    owner.publish_handoff()
    owner._fail_open()
    assert owner.publish_handoff()["acknowledgedOwner"] is None


def test_publishes_no_credentials_and_valid_atomic_json(tmp_path):
    owner = ready_owner(tmp_path)
    request(tmp_path)
    snapshot = owner.publish_handoff()
    assert json.loads((tmp_path / "status.json").read_text()) == snapshot
    assert set(snapshot) == {
        "version",
        "pid",
        "updatedAt",
        "state",
        "generation",
        "acknowledgedOwner",
        "lastPhysicalInputAt",
    }


def test_disabled_hooks_still_serialize_hardware_handoff(tmp_path):
    owner = ready_owner(tmp_path)
    owner.enabled = False
    token = owner.begin_unprotected_call()
    request(tmp_path)
    assert owner.publish_handoff()["acknowledgedOwner"] is None
    owner.end_call(token)
    assert owner.publish_handoff()["acknowledgedOwner"] == "robot-one"
    with pytest.raises(control.ControlBlocked, match="HARDWARE_CONTROL"):
        owner.begin_unprotected_call()
