"""End-to-end smoke: tiny training run + eval battery on a toy config."""

import numpy as np

from data import load_config, make_env_config
from evaluate import (eval_sc2_counterfactual, per_group_mse, predict,
                      target_tensor)
from env.dynamics import CauseLabels, N_ACTIONS
from train import train_one


def _tiny_cfg():
    cfg = load_config(
        __import__("pathlib").Path(__file__).parent.parent
        / "configs/default.yaml")
    cfg["data"]["episodes"] = 24
    cfg["data"]["val_episodes"] = 6
    cfg["env"]["episode_len"] = 32
    cfg["train"]["max_steps"] = 60
    cfg["eval"]["episodes"] = 6
    cfg["eval"]["adapt"]["steps"] = 20
    return cfg


def test_train_smoke(tmp_path):
    cfg = _tiny_cfg()
    res = train_one("mlp_state_action", 0, cfg, "cpu", tmp_path,
                    quiet=True)
    assert (tmp_path / "ckpt.pt").exists()
    assert np.isfinite(res["val_loss"])
    # even a tiny run should beat predicting "no change"
    ds = res["val_ds"]
    naive = float(((ds["next_obs"] - ds["obs"]) ** 2).mean())
    assert res["val_loss"] < naive * 5  # very loose wiring check


def test_eval_battery_keys(tmp_path):
    cfg = _tiny_cfg()
    res = train_one("gru_state_action", 0, cfg, "cpu", tmp_path,
                    quiet=True)
    model = res["model"]
    env_cfg = make_env_config(cfg["env"])
    from data import EVAL_SEED_OFFSET, collect_dataset
    ds = collect_dataset(env_cfg, cfg["eval"]["episodes"],
                         EVAL_SEED_OFFSET, env_seed=0)
    labels = ds["cause_labels"]
    pred = predict(model, ds, "cpu")
    g = per_group_mse(pred, target_tensor(ds, "cpu"), labels)
    assert set(g) == {"overall", "self", "external", "mixed", "noise"}

    cf = eval_sc2_counterfactual(model, env_cfg, 0, labels, "cpu",
                                 n_episodes=2, seed=0)
    assert "counterfactual_cos" in cf and cf["n_pairs"] > 0


def test_counterfactual_diff_isolated_to_action_dims():
    """Ground truth: alternative-action obs diffs are zero on ext/noise."""
    from env import AgencyEnv
    env = AgencyEnv(make_env_config(_tiny_cfg()["env"]), seed=0)
    env.reset()
    for _ in range(5):
        env.step(0)
    st = env.get_state()
    outs = {}
    for a in range(N_ACTIONS):
        env.set_state(st)
        outs[a], _ = env.step(a)
    lab = env.cause_labels
    nonself = ~np.isin(lab, [CauseLabels.SELF, CauseLabels.MIXED])
    for a in range(N_ACTIONS):
        np.testing.assert_allclose(
            (outs[a] - outs[0])[nonself], 0.0, atol=1e-12)
