"""CX0 organ modules — the validated mechanisms, re-instantiated for the
CTX world and frozen after Stage-A pretraining.

- T0Organ: GRU64 whose recurrent state carries elapsed-since-cue (the T0
  result: recurrence is causally required; interpolation OK, extrapolation
  weak). Signal = elapsed estimate + confidence + projected embedding.
- S0Organ: GRU64 action-conditioned forward model (the S0 result:
  action-aware prediction separates self-caused from world-caused change).
  Signal = error decomposition + learned self/world/noise attribution.
- H0Organ: GRU32 over interoceptive channels -> per-need urgency + drive
  embedding (the H0 result: internal state is causally used for regulation).
- R0Organ: content-addressed slot memory (the R0 result: content-based
  readout — slot permutation is a no-op). Store/recall DECISIONS belong to
  the integration layer; the memory mechanics are the organ.
- O0: reserved zeros — PARTIAL verdict, shadow-only by manifest. Not wired.
"""

from __future__ import annotations

import numpy as np
import torch
import torch.nn as nn

from ..memory.slot_memory import SlotMemory
from .base import OrganSignals


# ----------------------------------------------------------------------
# torch modules (also used by Stage-A pretraining)
# ----------------------------------------------------------------------
class T0Net(nn.Module):
    def __init__(self, hidden: int = 64, emb: int = 14):
        super().__init__()
        self.gru = nn.GRUCell(3, hidden)
        self.elapsed = nn.Linear(hidden, 1)   # steps since last cue / 96
        self.delay = nn.Linear(hidden, 1)     # last cue intensity (delay /96)
        self.proj = nn.Linear(hidden, emb)

    def forward(self, x, h):
        h = self.gru(x, h)
        return torch.sigmoid(self.elapsed(h)), torch.sigmoid(self.delay(h)), \
            torch.tanh(self.proj(h)), h


class S0Net(nn.Module):
    def __init__(self, hidden: int = 64, obs_dim: int = 16, n_act: int = 11):
        super().__init__()
        self.gru = nn.GRUCell(obs_dim + n_act, hidden)
        self.pred = nn.Linear(hidden, obs_dim)
        self.attr = nn.Linear(hidden + obs_dim + n_act, 3)  # self/world/noise

    def forward(self, obs, act_oh, h):
        h = self.gru(torch.cat([obs, act_oh], dim=-1), h)
        return self.pred(h), self.attr(torch.cat([h, obs, act_oh], dim=-1)), h


class H0Net(nn.Module):
    def __init__(self, hidden: int = 32):
        super().__init__()
        self.gru = nn.GRUCell(5, hidden)
        self.urgency = nn.Linear(hidden, 3)   # danger per crisis var (E,F,T)
        self.drive = nn.Linear(hidden, 2)

    def forward(self, x, h):
        h = self.gru(x, h)
        return torch.sigmoid(self.urgency(h)), torch.tanh(self.drive(h)), h


# ----------------------------------------------------------------------
# runtime organ wrappers (numpy-facing, hidden state kept internally)
# ----------------------------------------------------------------------
class T0Organ:
    name = "t0"

    def __init__(self, net: T0Net):
        self.net = net
        self.h = torch.zeros(1, net.gru.hidden_size)
        self._sig = np.zeros(18, dtype=np.float32)

    def reset(self, rng=None):
        self.h = torch.zeros(1, self.net.gru.hidden_size)
        self._sig = np.zeros(18, dtype=np.float32)

    @torch.no_grad()
    def update(self, cue: float, go: float, event_present: float):
        x = torch.tensor([[cue, go, event_present]], dtype=torch.float32)
        el, dl, emb, self.h = self.net(x, self.h)
        el_v = float(el.item()); dl_v = float(dl.item())
        # confidence = a cue has actually been decoded (delay channel active)
        conf = float(min(1.0, dl_v * 3.0))
        # due channel: rises as decoded elapsed approaches decoded delay
        due = 1.0 / (1.0 + float(np.exp(-((el_v - dl_v) * 80.0 / 3.0 - 0.5))))
        self._sig = np.concatenate(
            [[el_v, dl_v, conf, due], emb.squeeze(0).numpy()]).astype(np.float32)

    def signal(self) -> np.ndarray:
        return self._sig

    def reset_hidden(self):
        self.h = torch.zeros_like(self.h)


class S0Organ:
    name = "s0"

    def __init__(self, net: S0Net):
        self.net = net
        self.h = torch.zeros(1, net.gru.hidden_size)
        self._pred: np.ndarray | None = None
        self._ema_err = 0.05
        self._sig = np.zeros(8, dtype=np.float32)

    def reset(self, rng=None):
        self.h = torch.zeros(1, self.net.gru.hidden_size)
        self._pred = None
        self._sig = np.zeros(8, dtype=np.float32)

    @torch.no_grad()
    def predict(self, obs: np.ndarray, action: int):
        """Called BEFORE env.step: prediction of next obs."""
        x = torch.tensor(obs[None], dtype=torch.float32)
        a = torch.zeros(1, 11); a[0, action] = 1.0
        pred, attr_logits, self.h = self.net(x, a, self.h)
        self._pred = pred.squeeze(0).numpy()
        self._attr = attr_logits.squeeze(0).numpy()

    def update(self, obs_now: np.ndarray):
        """Called AFTER env.step: compare prediction with reality."""
        if self._pred is None:
            return
        err = np.abs(obs_now - self._pred)
        e_self = float(err[0:4].mean())
        e_ev = float(err[8:12].mean())
        e_world = float(err[4:8].mean() + err[12:14].mean()) / 2.0
        e_tot = float(err.mean())
        self._ema_err = 0.98 * self._ema_err + 0.02 * e_tot
        probs = _softmax(self._attr)
        conf = 1.0 / (1.0 + 10.0 * e_tot)
        self._sig = np.array(
            [probs[0], probs[1], probs[2], conf,
             e_self, e_ev, e_world, self._ema_err], dtype=np.float32)

    def signal(self) -> np.ndarray:
        return self._sig

    def reset_hidden(self):
        self.h = torch.zeros_like(self.h)


class H0Organ:
    name = "h0"

    def __init__(self, net: H0Net):
        self.net = net
        self.h = torch.zeros(1, net.gru.hidden_size)
        self._sig = np.zeros(9, dtype=np.float32)

    def reset(self, rng=None):
        self.h = torch.zeros(1, self.net.gru.hidden_size)
        self._sig = np.zeros(9, dtype=np.float32)

    @torch.no_grad()
    def update(self, internal: np.ndarray):
        x = torch.tensor(internal[None], dtype=torch.float32)
        urg, drv, self.h = self.net(x, self.h)
        u = urg.squeeze(0).numpy()
        # emergency flag: interoceptive bound violations (homeostatic alarm)
        e, f, t, _nov, cert = internal
        emerg = float(e < 0.30 or f > 0.80 or abs(t - 0.55) > 0.25
                      or cert < 0.15)
        self._sig = np.concatenate(
            [internal.astype(np.float32), [float(u.max()), emerg],
             drv.squeeze(0).numpy()]).astype(np.float32)

    def signal(self) -> np.ndarray:
        return self._sig

    def reset_hidden(self):
        self.h = torch.zeros_like(self.h)


class R0Organ:
    """Content-addressed slot memory. Decisions come from the policy;
    the organ owns only the mechanics + last-readout signal."""

    name = "r0"

    def __init__(self, memory: SlotMemory):
        self.memory = memory
        self._payload = np.zeros(8, dtype=np.float32)
        self._score = -1.0

    def reset(self, rng=None):
        self.memory.erase()
        self._payload = np.zeros(8, dtype=np.float32)
        self._score = -1.0

    def on_recall(self, payload: np.ndarray, score: float, slot: int = -1):
        self._payload = payload.astype(np.float32)
        self._score = score

    def signal(self) -> np.ndarray:
        avail = 1.0 if self._score > 0.3 else 0.0
        return np.concatenate(
            [self._payload, [avail], self.memory.summary()]).astype(np.float32)

    # interventions
    def erase(self):
        self.memory.erase()

    def shuffle(self, rng: np.random.Generator):
        self.memory.shuffle_payloads(rng)


def _softmax(x: np.ndarray) -> np.ndarray:
    e = np.exp(x - x.max())
    return e / e.sum()


# ----------------------------------------------------------------------
# organ set: orchestrates per-step updates and builds the bundle
# ----------------------------------------------------------------------
class OrganSet:
    """Holds all organs + slot memory; produces OrganSignals each step.

    Update order per env step:
      1. policy picks action from bundle_t
      2. s0.predict(obs_t, a_t)          (before env.step)
      3. env.step(a_t, memory)           (env executes STORE/RECALL via memory)
      4. organs.update(obs_{t+1}, internal_{t+1})  -> bundle_{t+1}
    """

    def __init__(self, t0: T0Organ, s0: S0Organ, h0: H0Organ, r0: R0Organ):
        self.t0, self.s0, self.h0, self.r0 = t0, s0, h0, r0
        self.memory = r0.memory
        self._bundle = OrganSignals.zeros()

    def reset(self, seed: int = 0):
        rng = np.random.default_rng(seed)
        for o in (self.t0, self.s0, self.h0, self.r0):
            o.reset(rng)
        self._bundle = OrganSignals.zeros()

    def pre_step(self, obs: np.ndarray, action: int):
        self.s0.predict(obs, action)

    def post_step(self, obs: np.ndarray, internal: np.ndarray,
                  recall_result: tuple[np.ndarray, float] | None) -> OrganSignals:
        if recall_result is not None:
            self.r0.on_recall(*recall_result)
        self.s0.update(obs)
        self.t0.update(cue=float(obs[12]), go=float(obs[13]),
                       event_present=float(obs[8]))
        self.h0.update(internal)
        self._bundle = OrganSignals(
            sensory=obs.astype(np.float32), h0=self.h0.signal(),
            s0=self.s0.signal(), t0=self.t0.signal(),
            r0=self.r0.signal(), o0=np.zeros(5, dtype=np.float32))
        return self._bundle

    @property
    def bundle(self) -> OrganSignals:
        return self._bundle
