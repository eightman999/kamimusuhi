"""CX0 architecture arms — C0 flat baseline, C1 generic GRU, C2 layered
recurrent stack, C3 modular sparse multi-timescale cortex ("Mio Cortical
Mantle v0").

All arms share: input encoder MLP(BUNDLE_DIM->enc), policy head (->11),
value head (->1), aux next-sensory prediction head (->16). Only the
recurrent core differs. `forward(x, state) -> (logits, value, aux, state,
pops)` where `pops` exposes named populations for probes/lesions.

C3 sparsity: block-diagonal recurrent masks (4 modules per population) plus
sparse long-range projections; per-population fixed leak rates implement
the plan's multi-timescale dynamics (sensory τ=1, assoc τ=4, context τ=16,
slow τ=64). Noise/Dale constraints are intentionally absent in v0 — they
gate on a C-series winner (plan §23).
"""

from __future__ import annotations

import numpy as np
import torch
import torch.nn as nn

from ..organs.base import BUNDLE_DIM

OBS_DIM = 16
N_ACTIONS = 11
ENC = 64


def count_params(m: nn.Module) -> int:
    return sum(p.numel() for p in m.parameters())


def _block_mask(n: int, n_blocks: int, cross_p: float,
                rng: np.random.Generator) -> torch.Tensor:
    """Block-diagonal dense + sparse cross-block long-range links."""
    m = np.zeros((n, n), dtype=np.float32)
    b = n // n_blocks
    for i in range(n_blocks):
        m[i * b:(i + 1) * b, i * b:(i + 1) * b] = 1.0
    cross = rng.random((n, n)) < cross_p
    cross[np.repeat(np.arange(n_blocks), b)[:, None] ==
          np.repeat(np.arange(n_blocks), b)[None, :]] = False
    m[cross] = 1.0
    np.fill_diagonal(m, 1.0)
    return torch.tensor(m)


class Heads(nn.Module):
    def __init__(self, core_dim: int):
        super().__init__()
        self.policy = nn.Linear(core_dim, N_ACTIONS)
        self.value = nn.Linear(core_dim, 1)
        self.aux = nn.Linear(core_dim, OBS_DIM)

    def forward(self, x):
        return self.policy(x), self.value(x).squeeze(-1), self.aux(x)


class BaseArm(nn.Module):
    name = "base"

    def __init__(self):
        super().__init__()
        self.enc = nn.Sequential(nn.Linear(BUNDLE_DIM, ENC), nn.Tanh(),
                                nn.Linear(ENC, ENC), nn.Tanh())

    def initial_state(self, batch: int = 1):
        raise NotImplementedError

    def core(self, e, state):
        raise NotImplementedError

    def forward(self, x, state):
        e = self.enc(x)
        c, state, pops = self.core(e, state)
        if self._lesioned:
            # true functional lesion: pop state AND its head contribution die
            if isinstance(state, dict):
                for k in self._lesioned:
                    if k in state:
                        state[k] = torch.zeros_like(state[k])
            elif torch.is_tensor(state) and "h" in self._lesioned:
                state = torch.zeros_like(state)
            for k in self._lesioned:
                if k in pops:
                    pops[k] = torch.zeros_like(pops[k])
            if pops:
                order = [k for k in ("context", "predict", "h") if k in pops]
                c = torch.cat([pops[k] for k in order], -1)
        if self._core_off:
            c = torch.zeros_like(c)
        logits, value, aux = self.heads(c)
        return logits, value, aux, state, pops

    _core_off: bool = False
    _lesioned: frozenset = frozenset()

    def set_core_off(self, v: bool) -> None:
        self._core_off = v

    def set_lesioned(self, pops) -> None:
        self._lesioned = frozenset(pops)


# ----------------------------------------------------------------------
class C0Flat(BaseArm):
    """Baseline: organs only, feedforward integration."""

    name = "c0"

    def __init__(self):
        super().__init__()
        self.mlp = nn.Sequential(nn.Linear(ENC, 128), nn.Tanh(),
                                 nn.Linear(128, 128), nn.Tanh())
        self.heads = Heads(128)

    def initial_state(self, batch: int = 1):
        return None

    def core(self, e, state):
        return self.mlp(e), None, {}


class C1GRU(BaseArm):
    """Parameter-matched generic GRU control."""

    name = "c1"

    def __init__(self, hidden: int = 117):   # 117 => ~76k params ≈ C3
        super().__init__()
        self.hidden = hidden
        self.gru = nn.GRUCell(ENC, hidden)
        self.heads = Heads(hidden)

    def initial_state(self, batch: int = 1):
        return torch.zeros(batch, self.hidden)

    def core(self, e, state):
        h = self.gru(e, state)
        return h, h, {"h": h}


class C2Layered(BaseArm):
    """Dense layered recurrent stack: assoc -> context -> predict,
    with a delayed feedback projection into context."""

    name = "c2"

    def __init__(self, a: int = 48, c: int = 64, p: int = 48, fb: int = 32):  # ~75k ≈ C3
        super().__init__()
        self.dims = dict(assoc=a, context=c, predict=p, feedback=fb)
        self.g_assoc = nn.GRUCell(ENC, a)
        self.g_context = nn.GRUCell(a + fb, c)
        self.g_predict = nn.GRUCell(c, p)
        self.feedback = nn.Linear(c, fb)
        self.heads = Heads(c + p)

    def initial_state(self, batch: int = 1):
        d = self.dims
        return dict(assoc=torch.zeros(batch, d["assoc"]),
                    context=torch.zeros(batch, d["context"]),
                    predict=torch.zeros(batch, d["predict"]),
                    feedback=torch.zeros(batch, d["feedback"]))

    def core(self, e, state):
        assoc = self.g_assoc(e, state["assoc"])
        context = self.g_context(torch.cat([assoc, state["feedback"]], -1),
                                 state["context"])
        predict = self.g_predict(context, state["predict"])
        feedback = torch.tanh(self.feedback(context))
        out = torch.cat([context, predict], -1)
        new = dict(assoc=assoc, context=context, predict=predict,
                   feedback=feedback)
        return out, new, {k: v for k, v in new.items() if k != "feedback"}


class LeakyPop(nn.Module):
    """Sparse-masked leaky-integrator recurrent population.
    h <- (1-a)h + a*tanh(Wx + (M*U)h + b); a = 1/tau fixed."""

    def __init__(self, n_in: int, n: int, tau: float,
                 mask: torch.Tensor | None = None):
        super().__init__()
        self.n = n
        self.register_buffer("alpha", torch.tensor(1.0 / tau))
        self.w_in = nn.Linear(n_in, n)
        self.w_rec = nn.Linear(n, n, bias=True)
        if mask is not None:
            self.register_buffer("mask", mask)
        else:
            self.mask = None

    def forward(self, x, h):
        w = self.w_rec.weight if self.mask is None else self.w_rec.weight * self.mask
        pre = self.w_in(x) + torch.addmm(self.w_rec.bias, h, w.t())
        return h + self.alpha * (torch.tanh(pre) - h)


class C3Mantle(BaseArm):
    """Modular sparse multi-timescale cortex (Mio Cortical Mantle v0).

    Populations and routing (hierarchy enforced by inputs, not labels):
      assoc   (48, τ=4)  <- encoder
      context (64, τ=16) <- assoc + feedback        (no raw input)
      slow    (32, τ=64) <- context (sparse proj)   (episode-scale)
      predict (48, τ=4)  <- context + assoc
      feedback(32, τ=8)  <- context + slow
    """

    name = "c3"

    TAUS = dict(assoc=4.0, context=16.0, slow=32.0, predict=4.0, feedback=8.0)

    def __init__(self, dims: dict | None = None, cross_p: float = 0.12,
                 seed: int = 0):
        super().__init__()
        d = dims or dict(assoc=64, context=96, slow=48, predict=64, feedback=48)
        self.dims = d
        rng = np.random.default_rng(seed)
        self.assoc = LeakyPop(ENC, d["assoc"], self.TAUS["assoc"],
                              _block_mask(d["assoc"], 4, cross_p, rng))
        self.context = LeakyPop(d["assoc"] + d["feedback"], d["context"],
                                self.TAUS["context"],
                                _block_mask(d["context"], 4, cross_p, rng))
        self.slow = LeakyPop(d["context"], d["slow"], self.TAUS["slow"],
                             _block_mask(d["slow"], 2, cross_p, rng))
        self.predict = LeakyPop(d["assoc"] + d["feedback"], d["predict"],
                                self.TAUS["predict"],
                                _block_mask(d["predict"], 4, cross_p, rng))
        self.feedback = LeakyPop(d["context"] + d["slow"], d["feedback"],
                                 self.TAUS["feedback"])
        self.heads = Heads(d["context"] + d["predict"] + d["assoc"])

    def initial_state(self, batch: int = 1):
        return {k: torch.zeros(batch, v) for k, v in self.dims.items()}

    def core(self, e, state):
        a = self.assoc(e, state["assoc"])
        c = self.context(torch.cat([a, state["feedback"]], -1), state["context"])
        s = self.slow(state["context"], state["slow"])
        p = self.predict(torch.cat([a, state["feedback"]], -1),
                         state["predict"])
        f = self.feedback(torch.cat([state["context"], state["slow"]], -1),
                          state["feedback"])
        new = dict(assoc=a, context=c, slow=s, predict=p, feedback=f)
        out = torch.cat([c, p, a], -1)
        return out, new, {k: new[k] for k in ("assoc", "context", "slow",
                                              "predict", "feedback")}


ARMS = {"c0": C0Flat, "c1": C1GRU, "c2": C2Layered, "c3": C3Mantle}


def build_arm(name: str, **kw) -> BaseArm:
    return ARMS[name](**kw)
