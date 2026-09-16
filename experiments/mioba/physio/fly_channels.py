"""A3.1 §3-§5, §19: Drosophila channel-family models.

These are NOT renamed generic Na/K (§4): each family carries its own
kinetic parameterisation, temperature scaling (Q10, §19), source, and
provenance. Kinetic parameters are LITERATURE_PRIOR summaries of
published Drosophila channel biophysics (Para/DmNav, Shab/Kv2,
Shaker/Kv1, Shal/Kv4); the HH *functional form* fitted to them is
MODEL_INFERENCE. Exact gating parameters vary across recordings —
the uncertainty is carried in ``confidence``/source, not hidden.

Gate form per family (voltage V in mV, rates in 1/ms):
    alpha = A / (1 + exp(-(V - Vh)/s))   or exp-form where noted
    tau_q10 = Q10 ** ((T_sim - T_ref)/10)   multiplies all rates

References for the parameter *ranges* (summarised, not digitised):
  - Para/DmNav: O'Dowd & Aldrich; fly Nav models — activation
    V1/2 ≈ -30..-45 mV across cell types (we use -40; the -30 end
    produced subthreshold depolarisation block under sustained
    drive), fast m (~0.05-0.2 ms), h ~1 ms, E_Na ≈ +50
  - Shab/Kv2: delayed rectifier — steep activation window near
    spike voltages (V1/2 ≈ -5, s=6) so it opens hard during the
    upstroke and disengages at rest; tau ~1-6 ms, no fast
    inactivation, E_K ≈ -77
  - Shaker/Kv1: fast transient A-type, act V1/2 ≈ -30, inact V1/2
    ≈ -60, tau_h ~5-15 ms
  - Shal/Kv4: fast transient, similar window, faster recovery
"""
from __future__ import annotations

import torch

from .channels import ChannelModel, _GateMixin

FLY_CHANNEL_MODEL_VERSION = "flychan-v1"

#: per-model kinetic parameter sets; each entry is itself a record —
#: (value, provenance) — assembled here as plain numbers with the
#: provenance declared once per model below.
def _gate(V, vh, s):
    """Boltzmann activation: open probability = 1/(1+exp(-(V-vh)/s))."""
    return 1.0 / (1.0 + torch.exp(-(V - vh) / s))


def _tau(V, tmin, tmax, vh, s):
    """Bell-shaped tau: tmin + (tmax-tmin)/(cosh((V-vh)/s))-ish.
    Smooth, bounded in [tmin, tmax] — MODEL_INFERENCE functional
    form (real taus are not exactly bell-shaped)."""
    return tmin + (tmax - tmin) / (2.0 * torch.cosh((V - vh) / s))


class _FlyChannel(_GateMixin, ChannelModel):
    """Shared base: Q10 temperature scaling on all gate rates."""
    family = "?"
    kinetics_provenance = "LITERATURE_PRIOR"
    form_provenance = "MODEL_INFERENCE"
    t_ref_C = 22.0

    def __init__(self, e_rev: float, temperature_C: float | None = None,
                 q10: float = 2.3):
        self.e_rev = float(e_rev)
        self.q10 = float(q10)
        self.temperature_C = (self.t_ref_C if temperature_C is None
                              else float(temperature_C))
        self._qt = self.q10 ** ((self.temperature_C - self.t_ref_C)
                                / 10.0)

    def _step(self, x, x_inf, tau, dt):
        tau = torch.clamp(tau / self._qt, min=1e-6)
        return self._cnexp(x, x_inf, tau, dt)


class ParaNa(_FlyChannel):
    """Para / DmNav — Drosophila voltage-gated Na channel.

    m^3 h kinetics with fly ranges: m V1/2 ~-40 mV (fast), h V1/2
    ~-55 mV. Rates summary of published fly Nav behaviour;
    LITERATURE_PRIOR windows, MODEL_INFERENCE exact curves."""
    name = "para_Na"
    family = "Para"
    e_rev_default = 50.0
    M_VH, M_S = -40.0, 8.0
    M_TMIN, M_TMAX = 0.04, 0.25
    H_VH, H_S = -55.0, -6.0
    H_TMIN, H_TMAX = 0.4, 2.5

    def __init__(self, e_rev: float | None = None, **kw):
        super().__init__(self.e_rev_default if e_rev is None else e_rev,
                         **kw)

    def initial_state(self, V):
        m = _gate(V, self.M_VH, self.M_S)
        h = _gate(V, self.H_VH, self.H_S)   # H_S<0 → h falls with V
        return torch.stack([m, h], dim=-1)

    def advance(self, dt, V, state):
        m, h = state[..., 0], state[..., 1]
        m = self._step(m, _gate(V, self.M_VH, self.M_S),
                       _tau(V, self.M_TMIN, self.M_TMAX, -45.0, 15.0),
                       dt)
        h = self._step(h, _gate(V, self.H_VH, self.H_S),
                       _tau(V, self.H_TMIN, self.H_TMAX, -55.0, 15.0),
                       dt)
        return torch.stack([m, h], dim=-1)

    def current(self, V, state):
        m, h = state[..., 0], state[..., 1]
        return (m ** 3) * h * (V - self.e_rev)

    def conductance(self, V, state):
        m, h = state[..., 0], state[..., 1]
        return (m ** 3) * h


class ShabK(_FlyChannel):
    """Shab / Kv2-like delayed rectifier — slow n kinetics, no fast
    inactivation. Fly DR carries the bulk of spike repolarisation."""
    name = "shab_K"
    family = "Shab"
    e_rev_default = -77.0
    N_VH, N_S = -5.0, 6.0
    N_TMIN, N_TMAX = 0.8, 6.0

    def __init__(self, e_rev: float | None = None, **kw):
        super().__init__(self.e_rev_default if e_rev is None else e_rev,
                         **kw)

    def initial_state(self, V):
        return _gate(V, self.N_VH, self.N_S).unsqueeze(-1)

    def advance(self, dt, V, state):
        n = self._step(state[..., 0], _gate(V, self.N_VH, self.N_S),
                       _tau(V, self.N_TMIN, self.N_TMAX, -20.0, 25.0),
                       dt)
        return n.unsqueeze(-1)

    def current(self, V, state):
        return (state[..., 0] ** 2) * (V - self.e_rev)

    def conductance(self, V, state):
        return state[..., 0] ** 2


class _AtypeK(_FlyChannel):
    """Shared m^1 h^1 A-type form for Shaker (Kv1) / Shal (Kv4)."""
    M_VH, M_S = -30.0, 9.0
    M_TMIN, M_TMAX = 0.5, 3.0
    H_VH, H_S = -60.0, -8.0
    H_TMIN, H_TMAX = 5.0, 20.0
    e_rev_default = -77.0

    def __init__(self, e_rev: float | None = None, **kw):
        super().__init__(self.e_rev_default if e_rev is None else e_rev,
                         **kw)

    def initial_state(self, V):
        m = _gate(V, self.M_VH, self.M_S)
        h = _gate(V, self.H_VH, self.H_S)
        return torch.stack([m, h], dim=-1)

    def advance(self, dt, V, state):
        m, h = state[..., 0], state[..., 1]
        m = self._step(m, _gate(V, self.M_VH, self.M_S),
                       _tau(V, self.M_TMIN, self.M_TMAX, -40.0, 15.0),
                       dt)
        h = self._step(h, _gate(V, self.H_VH, self.H_S),
                       _tau(V, self.H_TMIN, self.H_TMAX, -60.0, 20.0),
                       dt)
        return torch.stack([m, h], dim=-1)

    def current(self, V, state):
        m, h = state[..., 0], state[..., 1]
        return m * h * (V - self.e_rev)

    def conductance(self, V, state):
        return state[..., 0] * state[..., 1]


class ShakerK(_AtypeK):
    """Shaker / Kv1-like fast transient K (A-type)."""
    name = "shaker_K"
    family = "Shaker"


class ShalK(_AtypeK):
    """Shal / Kv4-like fast transient K."""
    name = "shal_K"
    family = "Shal"
    M_VH, M_S = -25.0, 9.0
    H_TMIN, H_TMAX = 4.0, 12.0


FLY_CHANNELS = {"para_Na": ParaNa, "shab_K": ShabK,
                "shaker_K": ShakerK, "shal_K": ShalK}

#: honest model-status note used in manifests
FLY_MODEL_NOTE = (
    "HH-form fits to published Drosophila channel behaviour; "
    "kinetic windows LITERATURE_PRIOR, exact curves MODEL_INFERENCE; "
    "not a molecular reconstruction")
