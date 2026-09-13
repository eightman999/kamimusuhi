"""End-to-end smoke: tiny train + full G0 battery for one v4 method."""

from pathlib import Path

import numpy as np

from ..config import load_config
from ..evaluate import eval_model
from ..train import train_one


def _cfg():
    return load_config(Path(__file__).parent.parent
                     / "configs/smoke.yaml")


def test_train_smoke_all_methods(tmp_path):
    cfg = _cfg()
    for method in ("gru", "cpc", "vicreg", "jepa"):
        res = train_one(method, 0, cfg, "cpu", tmp_path / method,
                        max_steps=30, quiet=True)
        d = tmp_path / method
        for f in ("latest.pt", "best.pt", "config.yaml", "meta.json",
                  "metrics.jsonl"):
            assert (d / f).exists(), f"{method}: {f}"
        assert np.isfinite(res["val_loss"]), method
        assert res["meta"]["data_keys"] == ["obs", "next_obs", "actions"]


def test_eval_keys_smoke(tmp_path):
    cfg = _cfg()
    res = train_one("jepa", 0, cfg, "cpu", tmp_path, max_steps=30,
                    quiet=True)
    ev = eval_model(res["model"], "jepa", cfg, 0, "cpu",
                    res["train_ds"], res["env_seed"])
    for k in ("probes", "clustering", "matching", "causal", "ood",
              "discrete", "intervention", "base_mse", "health"):
        assert k in ev, k
    for k in ("acc_in", "acc_loco", "acc_ood_ctx", "acc_dense_ctx"):
        assert k in ev["probes"], k
    # dynseg probes need >=40 single-cause segments — smoke data may not
    # reach that; check shape only when they ran
    if "dynseg_acc_in" in ev["probes"]:
        for k in ("dynseg_acc_loco", "dynseg_acc_ood_ctx"):
            assert k in ev["probes"], k
    for k in ("match_train_ctx", "match_ood_ctx", "match_ood_ctx_null"):
        assert k in ev["matching"], k
    for k in ("midctx_acc", "midctx_acc_null"):
        assert k in ev["causal"], k
    assert "combo_oodctx_auc" in ev["ood"]
    h = ev["health"]["main"]
    for k in ("var_mean", "effective_rank", "cos_abs_mean",
              "collapsed"):
        assert k in h, k
