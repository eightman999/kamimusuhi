"""End-to-end smoke: tiny train + eval battery on the smoke config."""

import numpy as np

from ..config import load_config
from ..data import train_val_datasets
from ..evaluate import eval_representation
from ..representations import TorchRep, build_analytic_rep
from ..train import train_one
from pathlib import Path


def _smoke_cfg():
    return load_config(Path(__file__).parent.parent
                     / "configs/smoke.yaml")


def test_train_smoke(tmp_path):
    cfg = _smoke_cfg()
    res = train_one("gru", 0, cfg, "cpu", tmp_path, quiet=True)
    assert (tmp_path / "ckpt.pt").exists()
    assert (tmp_path / "ckpt_best.pt").exists()
    assert np.isfinite(res["val_loss"])
    # beats predicting "no change" (delta target => naive baseline = var)
    ds = res["val_ds"]
    naive = float(((ds["next_obs"] - ds["obs"]) ** 2).mean())
    assert res["val_loss"] < naive * 5


def test_eval_keys_smoke(tmp_path):
    cfg = _smoke_cfg()
    res = train_one("gru_vq", 0, cfg, "cpu", tmp_path, quiet=True)
    rep = TorchRep("gru_vq", res["model"], "cpu")
    ev = eval_representation(rep, cfg, 0, "cpu", model=res["model"],
                             train_ds=res["train_ds"], env_seed=0)
    for k in ("probes", "clustering", "matching", "causal", "ood",
              "discrete", "intervention", "base_mse"):
        assert k in ev, k
    for k in ("acc_in", "acc_loco", "acc_ood_ctx", "acc_dense_ctx",
              "fewshot_in", "fewshot_ood", "best_action_acc"):
        assert k in ev["probes"], k
    for k in ("nmi_pooled", "nmi_ood_ctx"):
        assert k in ev["clustering"], k
    for k in ("match_train_ctx", "match_ood_ctx"):
        assert k in ev["matching"], k
    for k in ("shuffle_acc", "dropout_acc", "midctx_acc", "decoy_rate",
              "perm_acc", "perm_delta", "shuffle_aligned_acc",
              "shuffle_aligned_delta"):
        assert k in ev["causal"], k
    for k in ("combo_presence_auc", "combo_set_acc", "acc_noise",
              "acc_gain"):
        assert k in ev["ood"], k
    assert ev["discrete"] is not None


def test_analytic_rep_eval_smoke(tmp_path):
    cfg = _smoke_cfg()
    train_ds, _ = train_val_datasets(cfg, 0)
    rep = build_analytic_rep("pca", cfg.env.obs_dim, cfg.model, train_ds)
    ev = eval_representation(rep, cfg, 0, "cpu", train_ds=train_ds,
                             env_seed=0)
    assert "acc_in" in ev["probes"]
    assert ev["discrete"] is None
