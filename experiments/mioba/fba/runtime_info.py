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


def collect_runtime_info(backend: str | None = None) -> dict:
    info = {
        "gpu_model": None, "driver": None, "cuda_runtime": None,
        "compute_capability": None, "torch_version": None,
        "genn_version": None, "git_commit": _git_commit(),
        "backend": backend, "hostname": socket.gethostname(),
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "python_version": platform.python_version(),
        "platform": platform.platform(),
    }
    try:
        import torch
        info["torch_version"] = torch.__version__
        if torch.cuda.is_available():
            info["cuda_runtime"] = torch.version.cuda
            info["gpu_model"] = torch.cuda.get_device_name(0)
            cc = torch.cuda.get_device_capability(0)
            info["compute_capability"] = f"{cc[0]}.{cc[1]}"
    except Exception:
        pass
    try:
        import pygenn
        info["genn_version"] = getattr(pygenn, "__version__", None)
    except ImportError:
        pass
    try:
        out = subprocess.run(
            ["nvidia-smi", "--query-gpu=driver_version",
             "--format=csv,noheader"], capture_output=True, text=True,
            timeout=3)
        if out.returncode == 0:
            info["driver"] = out.stdout.strip().splitlines()[0]
    except (OSError, subprocess.SubprocessError):
        pass
    return info
