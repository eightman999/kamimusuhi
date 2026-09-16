"""A3 pluggable channel models (§8–§10).

``ChannelModel`` contract: ``current(V, state)`` → mA/cm^2 (outward
positive), ``advance(dt, V, state)`` → new state, ``initial_state(V)``.
All ops are batched tensors — one instance serves all nodes carrying
that channel.

A3 v0 ships the minimal active set (§8): leak, generic_Na, generic_Kdr.
These are Hodgkin–Huxley-type kinetics — *generic* models, NOT claimed
to reproduce specific Drosophila channels (§10). The interface accepts
later A-type K / Ca / KCa / HCN-style models.

Gate updates use exact single-step (cnexp) integration — stable at
dt ≤ 0.1 ms; the integrator name is recorded in run manifests (§22).
"""
from __future__ import annotations

import torch

CHANNEL_MODEL_VERSION = "chan-v0"


class ChannelModel:
    name = "abstract"

    def initial_state(self, V: torch.Tensor) -> torch.Tensor:
        raise NotImplementedError

    def advance(self, dt: float, V: torch.Tensor,
                state: torch.Tensor) -> torch.Tensor:
        raise NotImplementedError

    def current(self, V: torch.Tensor,
                state: torch.Tensor) -> torch.Tensor:
        """Outward-positive current density (mA/cm^2 per unit area).
        For density-bearing channels the caller multiplies by g_bar."""
        raise NotImplementedError

    def conductance(self, V: torch.Tensor,
                    state: torch.Tensor) -> torch.Tensor:
        """Effective conductance multiplier g_eff(V,state) such that
        current = g_bar * g_eff * (V - e_rev). Lets the integrator
        treat the channel semi-implicitly in V (§22)."""
        raise NotImplementedError

    e_rev: float = 0.0


class LeakChannel(ChannelModel):
    """I = g_leak * (V - E_leak) — stateless ohmic leak."""
    name = "leak"

    def __init__(self, e_rev: float = -54.4, e_leak: float | None = None):
        # e_leak accepted as an alias — callers use the uniform ``e_rev``
        self.e_rev = float(e_rev if e_leak is None else e_leak)

    def initial_state(self, V):
        return torch.empty((*V.shape, 0), device=V.device)

    def advance(self, dt, V, state):
        return state

    def current(self, V, state):
        return V - self.e_rev           # multiplied by g_leak outside

    def conductance(self, V, state):
        return torch.ones_like(V)


class _GateMixin:
    """cnexp update: x' = x_inf + (x - x_inf) * exp(-dt/tau)."""

    @staticmethod
    def _cnexp(x, x_inf, tau, dt):
        tau = torch.clamp(tau, min=1e-6)
        return x_inf + (x - x_inf) * torch.exp(-dt / tau)


class GenericNa(_GateMixin, ChannelModel):
    """HH-type fast sodium: g = gbar * m^3 * h, E_Na.

    Kinetics are the classic squid-axon HH rates scaled to a -60 mV
    rest — GENERIC model, not a fly channel (§10).
    """
    name = "generic_Na"

    def __init__(self, e_rev: float = 50.0, v_shift: float = 0.0):
        self.e_rev = float(e_rev)
        self.v_shift = float(v_shift)

    # rate helpers (V in mV, rates in 1/ms)
    def _am(self, V):
        x = (V - self.v_shift + 40.0) / 10.0
        return torch.where(x.abs() < 1e-6, torch.ones_like(x),
                           x / (1 - torch.exp(-x)))
    def _bm(self, V):
        return 4.0 * torch.exp(-(V - self.v_shift + 65.0) / 18.0)
    def _ah(self, V):
        return 0.07 * torch.exp(-(V - self.v_shift + 65.0) / 20.0)
    def _bh(self, V):
        return 1.0 / (1 + torch.exp(-(V - self.v_shift + 35.0) / 10.0))

    def initial_state(self, V):
        a, b = self._am(V), self._bm(V)
        m = a / (a + b)
        a, b = self._ah(V), self._bh(V)
        h = a / (a + b)
        return torch.stack([m, h], dim=-1)

    def advance(self, dt, V, state):
        m, h = state[..., 0], state[..., 1]
        a, b = self._am(V), self._bm(V)
        m = self._cnexp(m, a / (a + b), 1 / (a + b), dt)
        a, b = self._ah(V), self._bh(V)
        h = self._cnexp(h, a / (a + b), 1 / (a + b), dt)
        return torch.stack([m, h], dim=-1)

    def current(self, V, state):
        m, h = state[..., 0], state[..., 1]
        return (m ** 3) * h * (V - self.e_rev)

    def conductance(self, V, state):
        m, h = state[..., 0], state[..., 1]
        return (m ** 3) * h


class GenericKdr(_GateMixin, ChannelModel):
    """HH-type delayed-rectifier potassium: g = gbar * n^4, E_K."""
    name = "generic_Kdr"

    def __init__(self, e_rev: float = -77.0, v_shift: float = 0.0):
        self.e_rev = float(e_rev)
        self.v_shift = float(v_shift)

    def _an(self, V):
        x = (V - self.v_shift + 55.0) / 10.0
        return torch.where(x.abs() < 1e-6, torch.ones_like(x),
                           0.1 * x / (1 - torch.exp(-x)))
    def _bn(self, V):
        return 0.125 * torch.exp(-(V - self.v_shift + 65.0) / 80.0)

    def initial_state(self, V):
        a, b = self._an(V), self._bn(V)
        return (a / (a + b)).unsqueeze(-1)

    def advance(self, dt, V, state):
        n = state[..., 0]
        a, b = self._an(V), self._bn(V)
        n = self._cnexp(n, a / (a + b), 1 / (a + b), dt)
        return n.unsqueeze(-1)

    def current(self, V, state):
        return (state[..., 0] ** 4) * (V - self.e_rev)

    def conductance(self, V, state):
        return state[..., 0] ** 4


CHANNELS = {"leak": LeakChannel, "generic_Na": GenericNa,
            "generic_Kdr": GenericKdr}


def build_channel(record: dict) -> ChannelModel:
    """Overlay channel record → model instance."""
    cls = CHANNELS[record["channel_model"]]
    kw = {}
    if "e_rev" in record and record["e_rev"] is not None:
        kw["e_rev"] = float(record["e_rev"])
    return cls(**kw)
