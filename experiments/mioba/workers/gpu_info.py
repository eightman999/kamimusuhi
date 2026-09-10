"""GPU inventory bound to a CUDA device string.

``query_gpus()`` lists every GPU nvidia-smi can see. A worker started with
``--device cuda:N`` must only ever report *its* GPU, so
``gpu_identity(device)`` resolves the torch device index to one physical
GPU: via the torch device UUID when available (robust to
``CUDA_VISIBLE_DEVICES`` re-ordering), otherwise by nvidia-smi index.
``query_device_gpu(device)`` returns the live nvidia-smi row for that GPU
only (temperature / VRAM / utilization for heartbeats).
Everything degrades to ``None``/``[]`` on a host without nvidia-smi.
"""
from __future__ import annotations

import csv
import io
import shutil
import subprocess

_FIELDS = ("index", "uuid", "name", "memory.total", "memory.used",
           "utilization.gpu", "temperature.gpu", "driver_version")


def _num(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def query_gpus() -> list[dict]:
    if not shutil.which("nvidia-smi"):
        return []
    try:
        out = subprocess.run(
            ["nvidia-smi", f"--query-gpu={','.join(_FIELDS)}",
             "--format=csv,noheader,nounits"],
            capture_output=True, text=True, timeout=5, check=True)
    except (OSError, subprocess.SubprocessError):
        return []
    gpus = []
    for row in csv.reader(io.StringIO(out.stdout), skipinitialspace=True):
        if len(row) != len(_FIELDS):
            continue
        gpus.append({
            "index": int(row[0]),
            "uuid": row[1],
            "name": row[2],
            "memory_total_mb": _num(row[3]),
            "memory_used_mb": _num(row[4]),
            "utilization_pct": _num(row[5]),
            "temperature_c": _num(row[6]),
            "driver_version": row[7],
        })
    return gpus


def device_index(device: str | None) -> int | None:
    """'cuda:1' -> 1, 'cuda' -> 0, 'cpu'/None -> None."""
    if not device:
        return None
    dev = str(device)
    if not dev.startswith("cuda"):
        return None
    if ":" in dev:
        try:
            return int(dev.split(":", 1)[1])
        except ValueError:
            return None
    return 0


def _norm_uuid(u) -> str | None:
    if u is None:
        return None
    s = str(u).strip().lower()
    if s.startswith("gpu-"):
        s = s[4:]
    return s.replace("-", "") or None


def torch_device_props(device: str | None) -> dict:
    """Properties torch reports for ``device`` (None fields when torch or
    CUDA is unavailable or the device is not CUDA)."""
    out = {"gpu_index": device_index(device), "gpu_model": None,
           "compute_capability": None, "gpu_uuid": None,
           "vram_total_bytes": None, "torch_version": None,
           "cuda_runtime": None}
    try:
        import torch
    except ImportError:
        return out
    out["torch_version"] = torch.__version__
    idx = out["gpu_index"]
    if idx is None or not torch.cuda.is_available():
        return out
    try:
        props = torch.cuda.get_device_properties(idx)
    except (RuntimeError, AssertionError):
        return out
    out["cuda_runtime"] = torch.version.cuda
    out["gpu_model"] = props.name
    out["compute_capability"] = f"{props.major}.{props.minor}"
    out["vram_total_bytes"] = int(props.total_memory)
    try:
        out["gpu_uuid"] = str(props.uuid)
    except AttributeError:
        out["gpu_uuid"] = None
    return out


def match_gpu(gpus: list[dict], props: dict) -> dict | None:
    """The nvidia-smi row for the torch device: same UUID if torch gave
    one, else the row whose index equals the torch index."""
    want = _norm_uuid(props.get("gpu_uuid"))
    if want:
        for g in gpus:
            if _norm_uuid(g.get("uuid")) == want:
                return g
    idx = props.get("gpu_index")
    if idx is not None:
        for g in gpus:
            if g.get("index") == idx:
                return g
    return None


def gpu_identity(device: str | None, gpus: list[dict] | None = None) -> dict:
    """Immutable identity of the GPU behind ``device`` for the research
    record (worker registration, evaluation runtime_info)."""
    props = dict(torch_device_props(device) or {})
    if props.get("gpu_index") is None:
        props["gpu_index"] = device_index(device)
    gpus = query_gpus() if gpus is None else gpus
    smi = match_gpu(gpus, props) if props["gpu_index"] is not None else None
    ident = {
        "device": device,
        "gpu_index": props["gpu_index"],
        "gpu_uuid": props.get("gpu_uuid") or (smi or {}).get("uuid"),
        "gpu_model": props.get("gpu_model") or (smi or {}).get("name"),
        "compute_capability": props.get("compute_capability"),
        "vram_total_bytes": props.get("vram_total_bytes"),
        "driver": (smi or {}).get("driver_version") or (smi or {}).get("driver"),
        "cuda_runtime": props.get("cuda_runtime") or (smi or {}).get("cuda"),
        "torch_version": props.get("torch_version"),
        "nvidia_smi_index": (smi or {}).get("index"),
    }
    if ident["vram_total_bytes"] is None and smi and smi.get("memory_total_mb"):
        ident["vram_total_bytes"] = int(smi["memory_total_mb"] * 1024 * 1024)
    return ident


def query_device_gpu(device: str | None, identity: dict | None = None
                     ) -> dict | None:
    """Live nvidia-smi sample for this worker's GPU only."""
    gpus = query_gpus()
    if not gpus:
        return None
    ident = identity or gpu_identity(device, gpus)
    return match_gpu(gpus, {"gpu_uuid": ident.get("gpu_uuid"),
                            "gpu_index": ident.get("nvidia_smi_index")
                            if ident.get("nvidia_smi_index") is not None
                            else ident.get("gpu_index")})
