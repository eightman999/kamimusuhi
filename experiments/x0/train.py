"""X0 self-supervised binding training.

No pair labels anywhere: the only training signal is co-occurrence within
the lag window.  Losses (per ordered modality pair, vectorized over a batch
of scenes; candidates restricted to the same scene):

  binder     L = w_c * multi-positive InfoNCE over in-window tokens
                  + w_r * soft cross-modal reconstruction
                  (target = similarity-weighted mix of in-window candidates)
  bottleneck L = w_ae * within-modality AE recon
                  + w_x * min-over-window cross-modal recon
  indep_ae   L = within-modality AE recon

Validation metric = the same label-free losses on held-out scenes; the
best checkpoint is selected on it (selection.json documents the criterion).
"""

import argparse
import copy
import hashlib
import json
import os
import random
import subprocess
import time
from pathlib import Path

import numpy as np
import torch
from torch import nn

from experiments.x0.env.latent_cause import LatentCauseEnv
from experiments.x0.models import make_model
from experiments.x0.models.encoders import pair_key


def atomic(path, obj):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(obj, indent=2, allow_nan=False) + "\n")
    tmp.replace(path)


def tensor_hash(model):
    h = hashlib.sha256()
    for name, value in model.state_dict().items():
        h.update(name.encode())
        h.update(value.detach().cpu().numpy().tobytes())
    return h.hexdigest()


def save_checkpoint(path, model, optimizer, config, stage, step, **extra):
    assert stage in ("initial", "best", "final")
    p = Path(path)
    tmp = p.with_suffix(".tmp")
    torch.save(dict(model=model.state_dict(),
                    optimizer=optimizer.state_dict() if optimizer else None,
                    config=config, stage=stage, step=step,
                    model_sha256=tensor_hash(model), **extra), tmp)
    tmp.replace(p)


def collate(scenes, modalities, device="cpu"):
    """Stack all tokens of each modality across scenes -> tensors with a
    scene id per token (candidates never cross scene boundaries)."""
    out = {}
    for m in modalities:
        feats, times, sid, evs = [], [], [], []
        for si, sc in enumerate(scenes):
            em = sc.emissions[m]
            feats.append(em.feats)
            times.append(em.times)
            sid.append(np.full(len(em.times), si, dtype=np.int64))
            evs.append(em.events)
        out[m] = {
            "feats": torch.as_tensor(np.concatenate(feats), dtype=torch.float32,
                                     device=device),
            "times": torch.as_tensor(np.concatenate(times), device=device),
            "scene": torch.as_tensor(np.concatenate(sid), device=device),
            "events": np.concatenate(evs),
        }
    return out


def _window_mask(tok_m, tok_n, window):
    same = tok_m["scene"][:, None] == tok_n["scene"][None, :]
    dt = (tok_m["times"][:, None] - tok_n["times"][None, :]).abs()
    return same, same & (dt <= window)


def binder_losses(model, toks, spec, modalities):
    Z = {m: model.embed(m, t["feats"]) for m, t in toks.items()}
    contra, recon, nq = 0.0, 0.0, 0
    for m in modalities:
        for n in modalities:
            if m == n:
                continue
            same, inwin = _window_mask(toks[m], toks[n], spec.window)
            if not bool(same.any()):
                continue
            sim = (Z[m] @ Z[n].T) / spec.tau
            neg_inf = torch.tensor(-1e9, device=sim.device)
            sim_same = torch.where(same, sim, neg_inf)
            log_den = torch.logsumexp(sim_same, dim=1)
            sim_pos = torch.where(inwin, sim, neg_inf)
            has_pos = inwin.any(1)
            log_num = torch.logsumexp(sim_pos[has_pos], dim=1)
            contra = contra + (-(log_num - log_den[has_pos])).sum()
            nq += int(has_pos.sum())
            if spec.recon_weight > 0 and pair_key(m, n) in model.decoders:
                w = torch.softmax(sim_pos[has_pos], dim=1)
                target = w @ toks[n]["feats"]
                pred = model.decode(m, n, Z[m][has_pos])
                recon = recon + ((pred - target) ** 2).sum(-1).mean(0) * len(pred)
    return contra / max(1, nq), recon / max(1, nq)


def bottleneck_losses(model, toks, spec, modalities, cross_only=False):
    z_raw = {m: model.encoders[m](t["feats"]) for m, t in toks.items()}
    ae, cross, nq = 0.0, 0.0, 0
    for m in modalities:
        if len(toks[m]["feats"]) == 0:
            continue
        rec = model.decode_into(m, z_raw[m])
        ae = ae + ((rec - toks[m]["feats"]) ** 2).sum(-1).sum()
        nq += len(toks[m]["feats"])
    cross_n = 0
    if not cross_only:
        for m in modalities:
            for n in modalities:
                if m == n or len(toks[n]["feats"]) == 0:
                    continue
                same, inwin = _window_mask(toks[m], toks[n], spec.window)
                if not bool(inwin.any()):
                    continue
                pred = model.decode_into(n, z_raw[m])
                d2 = torch.cdist(pred, toks[n]["feats"]) ** 2
                d2 = torch.where(inwin, d2,
                                 torch.tensor(1e9, device=d2.device))
                has = inwin.any(1)
                cross = cross + d2[has].min(1).values.sum()
                cross_n += int(has.sum())
    return ae / max(1, nq), cross / max(1, cross_n)


def compute_losses(model, toks, spec, modalities):
    if spec.kind == "binder":
        contra, recon = binder_losses(model, toks, spec, modalities)
        total = spec.contra_weight * contra + spec.recon_weight * recon
        return total, {"contra": float(contra), "recon": float(recon)}
    ae, cross = bottleneck_losses(model, toks, spec, modalities,
                                  cross_only=(spec.kind == "indep_ae"))
    if spec.kind == "indep_ae":
        return ae, {"ae": float(ae)}
    total = spec.ae_weight * ae + spec.cross_weight * cross
    return total, {"ae": float(ae), "cross": float(cross)}


def eval_label_free(model, scenes, spec, modalities, device, chunk=128):
    model.eval()
    tot, wsum = 0.0, 0
    with torch.no_grad():
        for i in range(0, len(scenes), chunk):
            batch = scenes[i:i + chunk]
            toks = collate(batch, modalities, device)
            loss, _ = compute_losses(model, toks, spec, modalities)
            tot += float(loss) * len(batch)
            wsum += len(batch)
    model.train()
    return tot / max(1, wsum)


def make_env(cfg, seed):
    """cfg["env"] is an EnvConfig dataclass."""
    e = cfg["env"]
    return LatentCauseEnv(e.scene_params(), factors=e.factors,
                          holdout=e.holdout, modalities=e.modalities,
                          seed=seed)


def train(config, artifacts, run_id):
    torch.set_num_threads(int(config["train"].get("threads", 4)))
    seed = int(config["seed"])
    random.seed(seed)
    np.random.seed(seed)
    torch.manual_seed(seed)
    device = config["train"].get("device", "cpu")
    directory = Path(artifacts) / "runs" / run_id
    if (directory / "status.json").exists():
        raise ValueError("Refusing to overwrite existing run: " + run_id)
    directory.mkdir(parents=True, exist_ok=True)
    config = copy.deepcopy(config)
    config["source_commit"] = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], text=True).strip()
    import dataclasses
    atomic(directory / "config.json",
           {k: (dataclasses.asdict(v) if dataclasses.is_dataclass(v) else v)
            for k, v in config.items()})
    t0 = time.time()
    status = dict(run_id=run_id, model=config["model"]["kind"], seed=seed,
                  status="running", pid=os.getpid(), started_at=t0)

    def state(**kw):
        status.update(kw, timestamp=time.time())
        atomic(directory / "status.json", status)

    state()
    env_cfg = config["env"]
    modalities = tuple(env_cfg["modalities"])
    env = make_env(config, seed)
    dims = {m: env.transforms[m].out_dim for m in modalities}
    spec = config["model"]

    train_scenes = env.sample_scenes(env_cfg["n_train_scenes"],
                                     seed=10_000 + seed)
    val_scenes = env.sample_scenes(env_cfg["n_val_scenes"],
                                 seed=20_000 + seed)
    from experiments.x0.config import ModelSpec
    mspec = ModelSpec(**spec)
    model = make_model(mspec, dims).to(device)
    opt = torch.optim.Adam(model.parameters(), lr=config["train"]["lr"])
    save_checkpoint(directory / "initial.pt", model, opt, config, "initial", 0)
    val0 = eval_label_free(model, val_scenes, mspec, modalities, device)
    atomic(directory / "initial_validation.json", {"val_loss": val0})
    best = val0
    atomic(directory / "selection.json",
           dict(criterion="label-free val loss (contra+recon / ae+cross)",
                step=0, val_loss=val0))
    n_scenes = len(train_scenes)
    rng = np.random.default_rng(seed + 30_000)
    steps = int(config["train"]["steps"])
    bs = int(config["train"]["batch_scenes"])
    try:
        for step in range(1, steps + 1):
            start = time.perf_counter()
            idx = rng.choice(n_scenes, size=min(bs, n_scenes), replace=False)
            toks = collate([train_scenes[i] for i in idx], modalities, device)
            loss, parts = compute_losses(model, toks, mspec, modalities)
            opt.zero_grad(set_to_none=True)
            loss.backward()
            norm = nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            opt.step()
            row = dict(step=step, loss=float(loss.detach()),
                       grad_norm=float(norm),
                       elapsed=time.perf_counter() - start, **parts)
            if step % config["train"]["val_every"] == 0 or step == steps:
                val = eval_label_free(model, val_scenes, mspec, modalities,
                                      device)
                row["val_loss"] = val
                if val < best:
                    best = val
                    save_checkpoint(directory / "best.pt", model, opt, config,
                                    "best", step, val_loss=val)
                    atomic(directory / "selection.json",
                           dict(criterion="label-free val loss", step=step,
                                val_loss=val))
            with (directory / "metrics.jsonl").open("a") as f:
                f.write(json.dumps(row, allow_nan=False) + "\n")
            state(step=step, val_loss=row.get("val_loss"))
            print(json.dumps(dict(run_id=run_id, **row)), flush=True)
        save_checkpoint(directory / "final.pt", model, opt, config, "final",
                        steps)
        state(status="complete", finished_at=time.time(),
              wall_time=time.time() - t0, best_val=best)
    except BaseException as exc:
        state(status="failed", error=str(exc))
        raise


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--config", required=True, help="JSON run config")
    p.add_argument("--artifacts", required=True)
    p.add_argument("--run-id", required=True)
    a = p.parse_args()
    cfg = json.loads(Path(a.config).read_text())
    from experiments.x0.config import EnvConfig
    env = EnvConfig(**{k: (tuple(v) if isinstance(v, list) else v)
                       for k, v in cfg["env"].items()})
    config = {"seed": cfg["seed"], "env": env, "model": cfg["model"],
              "train": cfg["train"], "eval": cfg.get("eval", {})}
    train(config, a.artifacts, a.run_id)


if __name__ == "__main__":
    main()
