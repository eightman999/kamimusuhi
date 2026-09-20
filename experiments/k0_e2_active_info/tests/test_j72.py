"""Closed-loop interface fixtures, not evidence of trained-policy performance.

The deterministic numeric policy below intentionally uses returned information.
All transport is in-process; these tests never start or contact a J72 service.
"""
import math
import urllib.request

import pytest
import torch

from experiments.k0_e2_active_info.j72_eval import evaluate_closed_loop


class ReturnedInformationFixture(torch.nn.Module):
    """Observe response confidence/value; no access to environment or truth."""
    def initial_state(self, batch, device):
        return torch.full((batch, 1), .5, device=device)

    def forward(self, observation, state):
        state = torch.where((observation[:, 10] > .5)[:, None], observation[:, 9:10], state)
        answer = torch.where(state[:, 0] > .5, 4, 3)
        action = torch.where(observation[:, 10] == 0, 5, 0)
        action = torch.where(observation[:, 14] > .5, answer, action)
        logits = torch.zeros(observation.shape[0], 6, device=observation.device)
        logits.scatter_(1, action[:, None], 10.)
        return logits, torch.zeros(observation.shape[0], device=observation.device), state


@pytest.fixture(autouse=True)
def prohibit_network(monkeypatch):
    def blocked(*args, **kwargs):
        raise AssertionError("J72 interface fixture attempted real network access")
    monkeypatch.setattr(urllib.request, "urlopen", blocked)


def evaluate(backend="fixture", fault="correct", n=16):
    return evaluate_closed_loop(ReturnedInformationFixture(), "fixture://no-network", n=n,
                                seed=920001, backend=backend, fault=fault if backend != "scripted" else None)


def terminal_actions(result):
    return [episode["steps"][-1]["action"] for episode in result["episodes"]]


def test_scripted_and_parsed_correct_response_both_drive_final_success():
    scripted = evaluate(backend="scripted")
    external = evaluate()
    assert scripted["downstream_success"] == external["downstream_success"] == 1.
    assert scripted["accepted_gate_calls"] == external["accepted_gate_calls"] == 16
    assert scripted["calls"] == scripted["http_attempts"] == 0
    assert scripted["api_success"] is None
    assert external["calls"] == 16 and external["http_attempts"] == 0
    assert external["api_success"] == external["parse_success"] == external["semantic_correctness"] == 1.
    assert terminal_actions(scripted) == terminal_actions(external)
    assert external["finite_core_state"]
    assert all(call["source"] == "controlled_fault_fixture"
               for episode in external["episodes"] for call in episode["calls"])
    # This reactive fixture emits CALL again while waiting; only one is accepted.
    emitted = sum(step["action"] == 5 for episode in external["episodes"] for step in episode["steps"])
    assert emitted > external["accepted_gate_calls"]
    assert all(sum(step["cost"] for step in episode["steps"]) == pytest.approx(.05)
               for episode in external["episodes"])


def test_schema_valid_opposite_fact_changes_downstream_action_and_fails():
    correct, wrong = evaluate(), evaluate(fault="contradiction")
    assert wrong["api_success"] == wrong["parse_success"] == 1.
    assert wrong["semantic_correctness"] == wrong["downstream_success"] == 0.
    assert correct["downstream_success"] == 1.
    assert all(a != b for a, b in zip(terminal_actions(correct), terminal_actions(wrong)))
    for correct_episode, wrong_episode in zip(correct["episodes"], wrong["episodes"]):
        # Prefix is exactly paired, until information arrives through the parser.
        for a, b in zip(correct_episode["steps"][:3], wrong_episode["steps"][:3]):
            assert a["observation"] == b["observation"]
        assert correct_episode["steps"][3]["observation"][9] != wrong_episode["steps"][3]["observation"][9]
    assert wrong["finite_core_state"]


@pytest.mark.parametrize("fault,api_success,attempt_count", [
    ("timeout", 0., 1), ("http_503", 0., 1), ("malformed", 1., 2), ("empty", 1., 2),
])
def test_transport_and_schema_failures_remain_unknown_and_finite(fault, api_success, attempt_count):
    result = evaluate(fault=fault)
    correct = evaluate()
    assert result["accepted_gate_calls"] == 16
    assert result["api_success"] == api_success
    assert result["parse_success"] == result["semantic_correctness"] == 0.
    assert result["finite_core_state"] and math.isfinite(result["reward"])
    assert terminal_actions(result) == [3] * 16
    expected_success = sum(action == 3 for action in terminal_actions(correct)) / 16
    assert result["downstream_success"] == expected_success
    for episode in result["episodes"]:
        assert len(episode["calls"]) == 1
        call = episode["calls"][0]
        assert len(call["attempts"]) == attempt_count
        assert call["valid"] is False and call["confidence"] == 0.
        assert all(step["observation"][10] == 0. for step in episode["steps"])
        assert all(step["information"] is None for step in episode["steps"])
        assert all(math.isfinite(step["hidden_norm"]) for step in episode["steps"])


def test_slow_response_has_sixteen_step_delay_and_still_updates_core():
    result = evaluate(fault="slow", n=8)
    assert result["downstream_success"] == 1.
    assert result["response_latency_seconds"] > 0.
    assert "not wall-clock feedback" in result["timing_scope"]
    for episode in result["episodes"]:
        steps = episode["steps"]
        assert all(step["language_latency"] == 16 for step in steps)
        assert steps[0]["status"] == "CALL"
        assert steps[16]["status"] == "RESPONSE"
        assert all(step["observation"][10] == 0 for step in steps[:17])
        assert steps[17]["observation"][10] == 1.
        assert steps[-1]["action"] in (3, 4)


def test_gui_episode_trace_schema_is_numeric_and_excludes_ground_truth():
    result = evaluate(n=4)
    document = {"representatives": [{"run_id": "fixture-only", **result}], "complete": True}
    representative = document["representatives"][0]
    assert representative["run_id"] == "fixture-only"
    expected_fields = {"t", "observation", "action", "hidden_norm", "information", "status", "language_latency", "cost", "resource"}
    for episode in representative["episodes"]:
        assert isinstance(episode["episode_id"], int)
        assert isinstance(episode["success"], bool)
        assert len(episode["steps"]) == 48
        for step in episode["steps"]:
            assert set(step) == expected_fields
            assert len(step["observation"]) == 16
            assert all(isinstance(value, (int, float)) and math.isfinite(value) for value in step["observation"])
            assert 0 <= step["action"] < 6
            assert isinstance(step["status"], str)
            assert step["information"] is None or isinstance(step["information"], (int, float))
            assert not {"latent", "target", "true_target", "raw_text"} & step.keys()
