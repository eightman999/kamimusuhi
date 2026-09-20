import base64
import json

import numpy as np
import pytest

from ..env.c0_env import C0Config
from ..persistence.artifact import (ARTIFACT_KEYS, ITEM_KEYS, read_artifact,
                                    write_artifact)
from ..persistence.codec import (apply_condition, compress, decode_lossless,
                                 decode_payload, decompress,
                                 encode_lossless, flatten, mask_state,
                                 shuffle_state, state_spec_from)


def spec_mem():
    c = C0Config()
    return state_spec_from(
        ["hidden", "mem_payloads", "mem_keys", "mem_occupied",
         "mem_insert", "mem_clock"], c, 64)


def spec_gru():
    return state_spec_from(["hidden"], C0Config(), 64)


def sample_state(c=None):
    c = c or C0Config()
    rng = np.random.default_rng(0)
    return {
        "hidden": rng.normal(size=64).astype(np.float32),
        "mem_payloads": rng.normal(size=(c.memory_slots, c.payload_dim))
                        .astype(np.float32),
        "mem_keys": rng.integers(-1, c.num_keys, c.memory_slots),
        "mem_occupied": rng.random(c.memory_slots) < 0.5,
        "mem_insert": rng.integers(0, 200, c.memory_slots),
        "mem_clock": np.int64(123),
    }


def test_lossless_roundtrip():
    st = sample_state()
    out = decode_lossless(encode_lossless(st))
    for k, v in st.items():
        np.testing.assert_array_equal(out[k], v)


def test_compressed_budget_respected():
    st = sample_state()
    spec = spec_mem()
    for b in (16, 64, 256, 1024, 4096):
        blob = compress(st, spec, b)
        assert len(blob) <= b
    # lossless direction: big budget keeps everything (up to fp16)
    st2 = decompress(compress(st, spec, 8192), spec)
    np.testing.assert_allclose(st2["hidden"], st["hidden"], atol=1e-3)


def test_compressed_topk_keeps_largest():
    st = {"hidden": np.zeros(64, np.float32)}
    st["hidden"][5] = 9.0
    st["hidden"][9] = -7.0
    blob = compress(st, spec_gru(), 6 + 2 * 4)   # room for exactly 2
    out = decompress(blob, spec_gru())
    assert out["hidden"][5] == pytest.approx(9.0, abs=1e-3)
    assert out["hidden"][9] == pytest.approx(-7.0, abs=1e-3)
    assert out["hidden"][0] == 0.0


def test_condition_filtering():
    st = sample_state()
    spec = spec_mem()
    cold = decode_payload(apply_condition(st, "cold", spec, None), "cold", spec)
    assert cold == {}
    hid = decode_payload(apply_condition(st, "hidden", spec, None),
                         "hidden", spec)
    assert set(hid) == {"hidden"}
    mem = decode_payload(apply_condition(st, "memory", spec, None),
                         "memory", spec)
    assert set(mem) == {"mem_payloads", "mem_keys", "mem_occupied",
                        "mem_insert", "mem_clock"}
    full = decode_payload(apply_condition(st, "full", spec, None),
                          "full", spec)
    assert set(full) == set(st)


def test_artifact_schema_no_episode_or_answer(tmp_path):
    st = sample_state()
    payloads = [apply_condition(st, "full", spec_mem(), None)]
    p = tmp_path / "art.json"
    write_artifact(p, agent="mem64", condition="full", payloads=payloads,
                   env_cfg_dict={"episode_len": 160})
    doc = json.loads(p.read_text())
    assert set(doc) <= ARTIFACT_KEYS
    for it in doc["items"]:
        assert set(it) <= ITEM_KEYS
    blob = p.read_bytes()
    for banned in (b"episode_seed", b"ep_seed", b"answer", b"context",
                   b"label", b"key2val", b"value_of"):
        assert banned not in blob


def test_artifact_cannot_identify_episode(tmp_path):
    """Two episodes' artifacts differ only in payload bytes: no id/seed."""
    c = C0Config()
    st = sample_state(c)
    p = tmp_path / "a.json"
    write_artifact(p, agent="mem64", condition="full",
                   payloads=[encode_lossless(st)])
    blob = p.read_bytes()
    # no episode seed digits anywhere outside base64 payloads
    doc = json.loads(p.read_text())
    meta = {k: v for k, v in doc.items() if k != "items"}
    assert "seed" not in json.dumps(meta)
    assert "episode" not in json.dumps(meta)


def test_mask_and_shuffle_change_state():
    st = sample_state()
    spec = spec_mem()
    rng = np.random.default_rng(1)
    m = mask_state(st, 0.5, rng, spec)
    assert np.abs(flatten(m, spec)).sum() < np.abs(flatten(st, spec)).sum()
    s = shuffle_state(st, rng, spec)
    assert not np.allclose(s["hidden"], st["hidden"])
    assert set(s) == set(st)
