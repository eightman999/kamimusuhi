"""Collect host/runtime metadata. Every field is None-tolerant."""
from __future__ import annotations

import platform
import socket
import subprocess
from datetime import datetime, timezone


def _git_commit() -> str | None:
    try:
        out = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True,
                             text=True, timeout=5, cwd=_repo_root())
        return out.stdout.strip() or None
    except (OSError, subprocess.SubprocessError):
        return None


def _repo_root():
    from pathlib import Path
    return str(Path(__file__).resolve().parents[3])


def collect_runtime_info(backend: str | None = None,
                         device: str | None = None) -> dict:
    """Host + GPU runtime metadata for the research record.

    GPU fields describe the GPU behind ``device`` (``cuda:N``), never a
    fixed device 0: a ``cuda:1`` P100 worker reports the P100. With
    ``device=None`` on a CUDA host, ``cuda:0`` is described (legacy).
    """
    from ..workers.gpu_info import gpu_identity
    info = {
        "device": device,
        "gpu_index": None, "gpu_uuid": None,
        "gpu_model": None, "driver": None, "cuda_runtime": None,
        "compute_capability": None, "vram_total_bytes": None,
        "torch_version": None,
        "genn_version": None, "git_commit": _git_commit(),
        "backend": backend, "hostname": socket.gethostname(),
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "python_version": platform.python_version(),
        "platform": platform.platform(),
    }
    probe = device
    if probe is None:
        try:
            import torch
            info["torch_version"] = torch.__version__
            if torch.cuda.is_available():
                probe = "cuda:0"
        except ImportError:
            pass
    try:
        ident = gpu_identity(probe)
    except Exception:
        ident = {}
    for k in ("gpu_index", "gpu_uuid", "gpu_model", "compute_capability",
              "vram_total_bytes", "driver", "cuda_runtime"):
        if ident.get(k) is not None:
            info[k] = ident[k]
    if ident.get("torch_version"):
        info["torch_version"] = ident["torch_version"]
    try:
        import pygenn
        info["genn_version"] = getattr(pygenn, "__version__", None)
    except ImportError:
        pass
    if info["driver"] is None:
        try:
            out = subprocess.run(
                ["nvidia-smi", "--query-gpu=driver_version",
                 "--format=csv,noheader"], capture_output=True, text=True,
                timeout=3)
            if out.returncode == 0 and out.stdout.strip():
                info["driver"] = out.stdout.strip().splitlines()[0]
        except (OSError, subprocess.SubprocessError):
            pass
    return info
