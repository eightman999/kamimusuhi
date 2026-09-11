"""Shiu et al. (2024) LIF/alpha-synapse parameter defaults shared by the
development stage and every backend. Development resolves genome
parameter mutations against these defaults into absolute values so that a
backend never falls back to the unmutated constant.
"""
from __future__ import annotations

DEFAULT_PARAMS: dict[str, float] = dict(
    tauMem=20.0, tauSyn=5.0, tDelay=1.8, v0=-52.0, vReset=-52.0,
    vRest=-52.0, vThr=-45.0, tRefrac=2.2, dt=0.1,
    scalePoisson=250.0, wScale=0.275,
)


class UnsupportedAttachmentRegion(RuntimeError):
    """An attachment names an FBA0 region the backend cannot map to
    neurons. Backends raise this instead of substituting random wiring."""


def resolve_params(mutations: list, base: dict[str, float] | None = None
                   ) -> dict[str, float]:
    """Apply global-scope ParameterMutation-like records (attrs/keys
    ``path``, ``op``, ``value``, ``scope``) in order to a copy of base."""
    params = dict(base or DEFAULT_PARAMS)
    for m in mutations:
        get = (lambda k: getattr(m, k)) if not isinstance(m, dict) else m.get
        if get("scope") != "global" or get("path") not in params:
            continue
        op, val, path = get("op"), float(get("value")), get("path")
        if op == "scale":
            params[path] *= val
        elif op == "add":
            params[path] += val
        else:
            params[path] = val
    return params
