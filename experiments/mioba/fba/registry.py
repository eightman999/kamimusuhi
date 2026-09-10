"""FBA backend registry."""
from __future__ import annotations

from .backend import BackendUnavailable, FbaBackend

_BACKENDS = {"mock", "torch", "genn"}


def get_backend(name: str, **kwargs) -> FbaBackend:
    if name == "mock":
        from .mock_backend import MockBackend
        return MockBackend(**{k: v for k, v in kwargs.items()
                              if k in ("n_neurons", "connectivity")})
    if name == "torch":
        from .torch_backend import TorchBackend
        return TorchBackend(**{k: v for k, v in kwargs.items()
                               if k in ("data_dir", "synthetic",
                                        "synthetic_neurons", "connectivity",
                                        "runs_dir", "region_mode")})
    if name == "genn":
        from .genn_backend import GennBackend
        return GennBackend(**{k: v for k, v in kwargs.items()
                              if k in ("data_dir", "synthetic",
                                       "synthetic_neurons", "connectivity",
                                       "runs_dir")})
    raise BackendUnavailable(f"unknown backend: {name!r}")


def available_backends() -> dict:
    out = {}
    for name in sorted(_BACKENDS):
        try:
            get_backend(name)
            out[name] = {"available": True, "reason": None}
        except BackendUnavailable as exc:
            out[name] = {"available": False, "reason": str(exc)}
        except Exception as exc:  # e.g. import errors
            out[name] = {"available": False, "reason": f"{type(exc).__name__}: {exc}"}
    return out
