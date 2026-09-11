"""State <-> bytes codecs for the C0 persistence artifact.

Two encodings:

    lossless    self-describing per-field binary (dtype + shape + raw).
    compressed  budgeted top-|value| sparse encoding of the flattened
                state vector: [u16 n_keep][u32 n_dims][(u16 idx, fp16 val)*]
                total size <= budget bytes; needs the state spec
                (ordered field names/shapes/dtypes) to rebuild.

The spec is structural metadata (which fields exist and their shapes) --
it is fixed by the agent architecture and contains no episode content.
"""
from __future__ import annotations

import numpy as np

DT_F32, DT_I64, DT_BOOL = 0, 1, 2
_D2T = {DT_F32: np.float32, DT_I64: np.int64, DT_BOOL: np.bool_}
_T2D = {np.dtype(np.float32): DT_F32, np.dtype(np.float64): DT_F32,
        np.dtype(np.int64): DT_I64, np.dtype(np.int32): DT_I64,
        np.dtype(np.bool_): DT_BOOL, np.dtype(bool): DT_BOOL}
HEADER = 6          # u16 n_keep + u32 n_dims
ENTRY = 4           # u16 idx + fp16 val


def state_spec_from(fields: list[str], cfg, hidden: int) -> list[tuple]:
    """Ordered (name, shape, dtypecode) spec for a runner state."""
    spec = []
    for f in fields:
        if f == "hidden":
            spec.append((f, (hidden,), DT_F32))
        elif f == "mem_payloads":
            spec.append((f, (cfg.memory_slots, cfg.payload_dim), DT_F32))
        elif f in ("mem_keys", "mem_occupied", "mem_insert"):
            spec.append((f, (cfg.memory_slots,),
                        DT_BOOL if f == "mem_occupied" else DT_I64))
        elif f == "mem_clock":
            spec.append((f, (), DT_I64))
        else:
            raise KeyError(f"unknown state field {f}")
    return spec


def flatten(state: dict, spec: list[tuple]) -> np.ndarray:
    parts = []
    for name, shape, _dt in spec:
        a = np.asarray(state[name], dtype=np.float32) if name in state \
            else np.zeros(shape, dtype=np.float32)
        parts.append(a.reshape(-1))
    return np.concatenate(parts) if parts else np.zeros(0, np.float32)


def unflatten(vec: np.ndarray, spec: list[tuple]) -> dict:
    out, off = {}, 0
    for name, shape, dt in spec:
        n = int(np.prod(shape)) if shape else 1
        seg = vec[off:off + n].reshape(shape) if shape else vec[off]
        out[name] = np.asarray(seg, dtype=_D2T[dt]).copy()
        off += n
    return out


# ----------------------------------------------------------------------
def encode_lossless(state: dict) -> bytes:
    buf = bytearray()
    names = sorted(state.keys())
    buf.append(len(names))
    for name in names:
        a = np.asarray(state[name])
        nb = name.encode()
        buf.append(len(nb))
        buf += nb
        buf.append(_T2D[a.dtype])
        buf.append(a.ndim)
        for s in a.shape:
            buf += int(s).to_bytes(4, "little")
        buf += a.astype(_D2T[_T2D[a.dtype]]).tobytes()
    return bytes(buf)


def decode_lossless(blob: bytes) -> dict:
    p, state = 0, {}
    nfields = blob[p]; p += 1
    for _ in range(nfields):
        nl = blob[p]; p += 1
        name = blob[p:p + nl].decode(); p += nl
        dt = blob[p]; p += 1
        nd = blob[p]; p += 1
        shape = tuple(int.from_bytes(blob[p + 4 * i:p + 4 * i + 4], "little")
                      for i in range(nd)); p += 4 * nd
        a = np.frombuffer(blob, dtype=_D2T[dt],
                          count=int(np.prod(shape)) if shape else 1, offset=p)
        state[name] = a.reshape(shape).copy() if shape else a[0].copy()
        p += a.nbytes
    return state


# ----------------------------------------------------------------------
def compress(state: dict, spec: list[tuple], budget: int) -> bytes:
    """Top-|value| sparse fp16 encoding of the flat state vector."""
    vec = flatten(state, spec)
    n_dims = len(vec)
    n_keep = max(0, min(n_dims, (budget - HEADER) // ENTRY))
    if n_keep:
        idx = np.argpartition(-np.abs(vec), n_keep - 1)[:n_keep]
        idx = idx[np.argsort(idx)]                      # canonical order
    else:
        idx = np.zeros(0, dtype=np.int64)
    buf = bytearray()
    buf += n_keep.to_bytes(2, "little")
    buf += n_dims.to_bytes(4, "little")
    vals = vec[idx].astype(np.float16)
    for i, v in zip(idx, vals):
        buf += int(i).to_bytes(2, "little")
        buf += v.tobytes()
    assert len(buf) <= budget
    return bytes(buf)


def decompress(blob: bytes, spec: list[tuple]) -> dict:
    n_keep = int.from_bytes(blob[0:2], "little")
    n_dims = int.from_bytes(blob[2:6], "little")
    vec = np.zeros(n_dims, dtype=np.float32)
    p = HEADER
    for _ in range(n_keep):
        i = int.from_bytes(blob[p:p + 2], "little")
        v = np.frombuffer(blob, dtype=np.float16, count=1, offset=p + 2)[0]
        vec[i] = np.float32(v)
        p += ENTRY
    return unflatten(vec, spec)


# ----------------------------------------------------------------------
def apply_condition(state: dict, condition: str, spec: list[tuple],
                    budget: int | None) -> bytes:
    """state dict -> artifact payload bytes for a restore condition."""
    if condition == "cold":
        return encode_lossless({})
    if condition == "hidden":
        keep = {k: v for k, v in state.items() if k == "hidden"}
        return encode_lossless(keep)
    if condition == "memory":
        keep = {k: v for k, v in state.items() if k.startswith("mem_")}
        return encode_lossless(keep)
    if condition == "full":
        return encode_lossless(state)
    if condition == "compressed":
        assert budget is not None
        return compress(state, spec, budget)
    raise ValueError(f"unknown condition {condition}")


def decode_payload(payload: bytes, condition: str,
                   spec: list[tuple]) -> dict:
    if condition == "compressed":
        return decompress(payload, spec)
    return decode_lossless(payload)


# -- causal-test transforms on decoded state dicts ----------------------
def mask_state(state: dict, frac: float, rng: np.random.Generator,
               spec: list[tuple]) -> dict:
    """Zero a fraction of flattened state components (C-C2)."""
    vec = flatten(state, spec)
    n = len(vec)
    k = int(round(frac * n))
    if k:
        idx = rng.choice(n, size=k, replace=False)
        vec[idx] = 0.0
    return unflatten(vec, spec)


def shuffle_state(state: dict, rng: np.random.Generator,
                  spec: list[tuple]) -> dict:
    """Permute hidden dims and memory slot order (C-C3)."""
    out = {k: np.asarray(v).copy() for k, v in state.items()}
    if "hidden" in out:
        perm = rng.permutation(len(out["hidden"].reshape(-1)))
        out["hidden"] = out["hidden"].reshape(-1)[perm].reshape(
            out["hidden"].shape)
    if "mem_payloads" in out:
        m = len(out["mem_payloads"])
        order = rng.permutation(m)
        for f in ("mem_payloads", "mem_keys", "mem_occupied", "mem_insert"):
            if f in out:
                out[f] = out[f][order]
    return out
