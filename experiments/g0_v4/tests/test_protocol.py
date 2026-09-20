"""Protocol validator tests: the firewall must hold and detect leaks."""

from pathlib import Path


from ..config import load_config
from ..protocol_validator import (check_eval_isolation,
                                  check_label_access, check_scope,
                                  check_run)
from ..train import TRAIN_KEYS, sanitize
from experiments.g0.data import train_val_datasets

SRC = Path(__file__).parent.parent


def _cfg():
    return load_config(SRC / "configs/smoke.yaml")


def test_sanitize_whitelist():
    cfg = _cfg()
    ds, _ = train_val_datasets(cfg, 0)
    s = sanitize(ds)
    assert set(s.keys()) == set(TRAIN_KEYS)
    for k in ("cause_a", "ctx", "seg_id", "canonical", "signal"):
        assert k not in s


def test_scope_and_eval_isolation_smoke():
    cfg = _cfg()
    assert check_scope(cfg, 0)["pass"]
    assert check_eval_isolation(cfg, 0)["pass"]


def test_label_access_scan_clean_and_detects_leak(tmp_path):
    # the real source tree must be clean
    res = check_label_access(SRC)
    assert res["pass"], res.get("hits")
    # a planted violation must be caught
    bad = tmp_path / "models.py"
    bad.write_text('def compute_loss(obs, nxt, act, ds):\n'
                   '    return ds["cause_a"].sum() * 0.0\n')
    res2 = check_label_access(tmp_path)
    assert not res2["pass"] and res2["hits"]


def test_check_run_flags_cpu_fallback(tmp_path):
    (tmp_path / "latest.pt").write_bytes(b"x")
    (tmp_path / "best.pt").write_bytes(b"x")
    (tmp_path / "config.yaml").write_text("seed: 0\n")
    (tmp_path / "meta.json").write_text(
        '{"method": "cpc", "seed": 0, "device": "cpu", "status": "ok",'
        ' "data_keys": ["obs", "next_obs", "actions"],'
        ' "oom_retries": 0}')
    (tmp_path / "metrics.jsonl").write_text(
        '{"step": 1, "train": 1.0, "val": 1.0}\n')
    res = check_run(tmp_path, want_device="cuda")
    assert not res["pass"]
    res2 = check_run(tmp_path, want_device=None)
    assert res2["pass"]
