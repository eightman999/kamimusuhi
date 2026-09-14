"""Stage A — organ pretraining for CX0.

Each organ is trained on its own validated subtask instantiated against the
CTX world, then frozen. Checkpoints are trained ONCE (fixed organ seed) and
shared by every architecture arm and integration seed — the plan's "same
organ checkpoints for all individuals" rule.

- T0Organ: synthetic cue-pulse streams; elapsed-since-cue + encoded-delay
  regression (the T0-A interval task interface).
- S0Organ: forward-model rollouts on ctx1/ctx2/ctx4 under a mixed
  random/oracle policy; next-obs MSE + self/world/noise attribution CE
  (labels from env.cause_labels / phantom flag — pretraining only).
- H0Organ: internal-state rollouts under oracle policy; per-need danger
  regression (env.danger_levels()).

Everything runs on CPU; the models are small (GRU32/64).
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn

from ..env.ctx_world import CtxWorld, EP_LEN, N_ACTIONS, Oracle
from ..memory.slot_memory import SlotMemory
from .modules import H0Net, H0Organ, OrganSet, R0Organ, S0Net, S0Organ, T0Net, T0Organ

ORGAN_SEED = 777


# ----------------------------------------------------------------------
# T0 — synthetic pulse streams
# ----------------------------------------------------------------------
def _t0_batch(rng, batch: int, seq: int = EP_LEN):
    """Inputs (B,T,3): cue pulse, go, event flag. Targets: elapsed/96,
    delay/96 (last pulse intensity)."""
    x = np.zeros((batch, seq, 3), dtype=np.float32)
    el = np.zeros((batch, seq, 1), dtype=np.float32)
    dl = np.zeros((batch, seq, 1), dtype=np.float32)
    for b in range(batch):
        n_pulses = int(rng.integers(0, 3))   # include no-cue sequences
        last_t, last_d = -1, 0.0
        pulses = rng.choice(np.arange(2, seq - 8), size=n_pulses, replace=False)
        for t in range(seq):
            if t in pulses:
                d = float(rng.uniform(0.25, 0.95))
                x[b, t, 0] = d
                last_t, last_d = t, d
            if rng.random() < 0.05:
                x[b, t, 1] = 1.0
            if rng.random() < 0.10:
                x[b, t, 2] = 1.0
            if last_t >= 0:
                el[b, t, 0] = (t - last_t) / EP_LEN
                dl[b, t, 0] = last_d
    return (torch.tensor(x), torch.tensor(el), torch.tensor(dl))


def pretrain_t0(epochs: int = 800, seed: int = ORGAN_SEED, device="cpu") -> T0Net:
    torch.manual_seed(seed)
    rng = np.random.default_rng(seed)
    net = T0Net().to(device)
    opt = torch.optim.Adam(net.parameters(), lr=3e-3)
    for ep in range(epochs):
        x, el, dl = _t0_batch(rng, 64)
        h = torch.zeros(64, 64)
        loss = 0.0
        for t in range(x.shape[1]):
            el_h, dl_h, _emb, h = net(x[:, t].to(device), h.to(device))
            loss = loss + nn.functional.mse_loss(el_h, el[:, t].to(device)) \
                + nn.functional.mse_loss(dl_h, dl[:, t].to(device))
        opt.zero_grad(); loss.backward(); opt.step()
    return net


# ----------------------------------------------------------------------
# S0 — forward model + attribution on env rollouts
# ----------------------------------------------------------------------
def _collect_rollouts(tasks, n_eps: int, rng, policy_mix: float = 0.5):
    """Returns list of (obs_seq (T,16), act_seq (T,), attr_labels (T,))."""
    data = []
    for i in range(n_eps):
        task = tasks[i % len(tasks)]
        w = CtxWorld(task, seed=int(rng.integers(100000)))
        mem = SlotMemory(3, 8, 6)
        orc = Oracle(task); orc.reset()
        obs = w._obs()
        obs_seq, act_seq, lab_seq = [obs.copy()], [], []
        for t in range(EP_LEN):
            if w.dead:
                break
            a = orc.act(w, mem) if rng.random() < policy_mix else int(rng.integers(N_ACTIONS))
            et, _st, phantom = w._perceived_event()
            info = w.step(a, mem)
            obs_seq.append(info.obs.copy())
            act_seq.append(a)
            # attribution label: was the perceived EVENT self-caused?
            # (supervised only on event-perception steps; else "noise")
            if phantom:
                lab = 0
            elif et:
                lab = 1
            else:
                lab = 2
            lab_seq.append(lab)
            obs = info.obs
        data.append((np.stack(obs_seq), np.array(act_seq), np.array(lab_seq)))
    return data


def pretrain_s0(n_eps: int = 400, epochs: int = 4, seed: int = ORGAN_SEED,
                device="cpu") -> S0Net:
    torch.manual_seed(seed)
    rng = np.random.default_rng(seed)
    data = _collect_rollouts(["ctx1", "ctx2", "ctx4"], n_eps, rng)
    net = S0Net().to(device)
    opt = torch.optim.Adam(net.parameters(), lr=3e-3)
    for ep in range(epochs):
        order = rng.permutation(len(data))
        tot = 0.0
        for i in order:
            obs_seq, act_seq, lab_seq = data[i]
            T = len(act_seq)
            h = torch.zeros(1, 64, device=device)
            loss = 0.0
            for t in range(T):
                obs_t = torch.tensor(obs_seq[t][None], dtype=torch.float32, device=device)
                act_oh = torch.zeros(1, N_ACTIONS, device=device)
                act_oh[0, act_seq[t]] = 1.0
                pred, attr, h = net(obs_t, act_oh, h)
                tgt = torch.tensor(obs_seq[t + 1][None], dtype=torch.float32, device=device)
                loss = loss + nn.functional.mse_loss(pred, tgt) \
                    + 0.3 * nn.functional.cross_entropy(
                        attr, torch.tensor([lab_seq[t]], device=device))
            opt.zero_grad(); loss.backward(); opt.step()
            tot += float(loss.detach())
    return net


# ----------------------------------------------------------------------
# H0 — danger regression on oracle rollouts
# ----------------------------------------------------------------------
def pretrain_h0(n_eps: int = 300, epochs: int = 4, seed: int = ORGAN_SEED,
                device="cpu") -> H0Net:
    torch.manual_seed(seed)
    rng = np.random.default_rng(seed)
    net = H0Net().to(device)
    opt = torch.optim.Adam(net.parameters(), lr=3e-3)
    seqs = []
    for i in range(n_eps):
        task = ["ctx1", "ctx4", "ctx5"][i % 3]
        w = CtxWorld(task, seed=int(rng.integers(100000)))
        mem = SlotMemory(3, 8, 6)
        orc = Oracle(task); orc.reset()
        ints, labs = [], []
        for t in range(EP_LEN):
            if w.dead:
                break
            a = orc.act(w, mem) if rng.random() < 0.8 else int(rng.integers(N_ACTIONS))
            info = w.step(a, mem)
            ints.append(info.internal.copy())
            labs.append(w.danger_levels())
        seqs.append((np.stack(ints), np.stack(labs)))
    for ep in range(epochs):
        for ints, labs in seqs:
            h = torch.zeros(1, 32, device=device)
            loss = 0.0
            for t in range(len(ints)):
                x = torch.tensor(ints[t][None], dtype=torch.float32, device=device)
                urg, _drv, h = net(x, h)
                loss = loss + nn.functional.mse_loss(
                    urg, torch.tensor(labs[t][None], dtype=torch.float32, device=device))
            opt.zero_grad(); loss.backward(); opt.step()
    return net


# ----------------------------------------------------------------------
# build / load
# ----------------------------------------------------------------------
def build_organ_set(ckpt_dir: str | Path, device="cpu") -> OrganSet:
    """Load frozen organ checkpoints; train them if missing."""
    ckpt_dir = Path(ckpt_dir)
    ckpt_dir.mkdir(parents=True, exist_ok=True)
    t0_p, s0_p, h0_p = (ckpt_dir / f"{n}.pt" for n in ("t0", "s0", "h0"))
    if not (t0_p.exists() and s0_p.exists() and h0_p.exists()):
        nets = pretrain_all(device=device)
        torch.save(nets["t0"].state_dict(), t0_p)
        torch.save(nets["s0"].state_dict(), s0_p)
        torch.save(nets["h0"].state_dict(), h0_p)
        (ckpt_dir / "meta.json").write_text(json.dumps(
            {"organ_seed": ORGAN_SEED, "frozen": True}, indent=2))
    t0, s0, h0 = T0Net(), S0Net(), H0Net()
    t0.load_state_dict(torch.load(t0_p, weights_only=True))
    s0.load_state_dict(torch.load(s0_p, weights_only=True))
    h0.load_state_dict(torch.load(h0_p, weights_only=True))
    for n in (t0, s0, h0):
        n.eval()
    return OrganSet(T0Organ(t0), S0Organ(s0), H0Organ(h0),
                    R0Organ(SlotMemory(3, 8, 6)))


def pretrain_all(device="cpu") -> dict:
    return {"t0": pretrain_t0(device=device),
            "s0": pretrain_s0(device=device),
            "h0": pretrain_h0(device=device)}
