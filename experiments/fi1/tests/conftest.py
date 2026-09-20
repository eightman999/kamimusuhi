import json

import pytest

from experiments.fi1.protocol.schema import make_turn
from experiments.fi1.runtime.engine import TurnEngine
from experiments.fi1.runtime.mocks import MemorySink


@pytest.fixture
def db_path(tmp_path):
    return tmp_path / "fi1.db"


@pytest.fixture
def sink():
    return MemorySink()


@pytest.fixture
def engine(db_path, sink):
    e = TurnEngine(db_path, sink=sink, seed=0)
    yield e
    e.close()


@pytest.fixture
def make():
    """make_turn shortcut; counter keeps turn_ids unique per test."""
    counter = {"n": 0}

    def _make(**kw):
        counter["n"] += 1
        kw.setdefault("payload", {"kind": "user_message",
                                  "text": f"msg-{counter['n']}"})
        kw.setdefault("timestamp", 1_700_000_000.0 + counter["n"])
        kw.setdefault("sequence", counter["n"])
        return make_turn(**kw)

    return _make


@pytest.fixture
def raw(make):
    def _raw(**kw):
        return json.dumps(make(**kw)).encode("utf-8")
    return _raw
