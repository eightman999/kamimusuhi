"""F2 separate-split data boundary and outcome-independent input interventions."""
from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path

import numpy as np
import torch

SCHEMA = "k0-f2-policy-v1"
TASK_DIM = 4
BODY_DIM = 20
ACTION_NAMES = ("RUN_CPU", "RUN_RTX3060", "RUN_P100", "WAIT")
PRIMARY_MODES = ("BODY", "BLIND", "SHUFFLED", "STALE")
TASKS = ((128, 8, .003), (512, 16, .008), (1024, 16, .025), (2048, 8, .07))
SPLIT_SESSIONS = {"train": {"A", "B"}, "validation": {"C"}, "test": {"D"}}
TEMPORAL_OOD = ("sampling_interval_2x", "body_update_delay", "network_jitter",
                "temporal_sensor_dropout", "stale_frames", "task_start_timing")
SENSOR_OOD = ("sensor_noise", "sensor_dropout", "constant_value", "scaled_telemetry",
              "partial_sensor_inversion", "sensor_permutation", "rtx3060_sensor_unavailable",
              "p100_sensor_unavailable", "mac_unavailable", "network_latency_increase")
RESOURCE_OOD = ("controlled_rtx_unavailable", "controlled_p100_unavailable", "controlled_cpu_constrained")


def sha256(path: Path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def json_write(path: Path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n")
    temporary.replace(path)


def task_features(task_id):
    n, repetitions, deadline = TASKS[task_id]
    return [n / 2048, repetitions / 16, deadline / .1, 3 * n * n * 4 / (64 * 1024**2)]


def validate_rows(rows, expected_split=None, lock_sha256=None, normalization_identity=None,
                  require_complete=False):
    if not rows:
        raise ValueError("empty F2 dataset")
    identifiers, block_sessions, combinations = set(), {}, {}
    for row in rows:
        if row.get("schema_version") != SCHEMA:
            raise ValueError("F2 fresh dataset schema required; K0-F data are forbidden")
        if require_complete and row.get("fixture_only"):
            raise ValueError("synthetic test fixtures cannot enter a confirmatory run")
        split, session = row["split"], row["session_id"]
        if split not in SPLIT_SESSIONS or session not in SPLIT_SESSIONS[split]:
            raise ValueError("session/split contamination")
        if expected_split is not None and split != expected_split:
            raise ValueError("a separate file must contain only its declared split")
        if lock_sha256 is not None and row.get("protocol_lock_sha256") != lock_sha256:
            raise ValueError("dataset protocol lock mismatch")
        if row["episode_id"] in identifiers:
            raise ValueError("duplicate episode identifier")
        identifiers.add(row["episode_id"])
        block = row["block_id"]
        if block in block_sessions and block_sessions[block] != (session, split):
            raise ValueError("block crosses session/split")
        block_sessions[block] = (session, split)
        if type(row["task_id"]) is not int or row["task_id"] not in range(4):
            raise ValueError("unknown fixed task")
        features = np.asarray(row["task_features"], dtype=float)
        if features.shape != (4,) or not np.allclose(features, task_features(row["task_id"]), atol=1e-7, rtol=0):
            raise ValueError("task input does not match the locked physical task")
        if row["deadline_seconds"] != TASKS[row["task_id"]][2]:
            raise ValueError("deadline differs from fixed task")
        timestamp = row["timestamp"]
        if not math.isfinite(timestamp):
            raise ValueError("nonfinite timestamp metadata")
        provenance = row["provenance"]
        if provenance.get("telemetry_kind") != "real":
            raise ValueError("primary data require real fresh telemetry")
        if normalization_identity is not None and provenance.get("normalization_identity") != normalization_identity:
            raise ValueError("normalization identity mismatch")
        for prefix in ("", "stale_"):
            value = np.asarray(row[prefix + "body_sequence"], dtype=float)
            mask = np.asarray(row[prefix + "body_mask_sequence"], dtype=float)
            if value.ndim != 2 or value.shape[1] != 20 or not 1 <= len(value) <= 8 or mask.shape != value.shape:
                raise ValueError("body/mask sequence must be [1..8,20]")
            if not np.isfinite(value).all() or not np.isfinite(mask).all() or np.any((value < 0) | (value > 1)) or np.any((mask < 0) | (mask > 1)):
                raise ValueError("body and mask must be finite normalized values")
            if not np.isin(mask, [0, 1]).all() or np.any(value[mask == 0] != 0):
                raise ValueError("raw frame masks must be binary and missing values neutral zero")
            frame_ids = provenance[prefix + "frame_ids"]
            times = provenance[prefix + "frame_timestamps"]
            if len(frame_ids) != len(value) or len(times) != len(value):
                raise ValueError("frame provenance does not align with sequence")
            limit = timestamp - (30 if prefix else 0)
            if any(not math.isfinite(t) or t > limit + 1e-6 for t in times) or times != sorted(times):
                raise ValueError("future telemetry or invalid stale timestamp")
        if len(row["body_sequence"]) != len(row["stale_body_sequence"]):
            raise ValueError("primary STALE must preserve the current history length")
        costs = np.asarray(row["costs_seconds"], dtype=float)
        if costs.shape != (4,) or not np.isfinite(costs).all() or np.any(costs <= 0):
            raise ValueError("four positive finite measured action costs required")
        if len(row["failures"]) != 4 or any(type(v) is not bool for v in row["failures"]):
            raise ValueError("four literal boolean failure labels required")
        combinations.setdefault(block, []).append((row["task_id"], len(row["body_sequence"])))
    if require_complete:
        if expected_split is None:
            raise ValueError("complete-design validation requires a declared split")
        sessions = {r["session_id"] for r in rows}
        if sessions != SPLIT_SESSIONS[expected_split]:
            raise ValueError("incomplete collection sessions")
        for session in sessions:
            if sum(v[0] == session for v in block_sessions.values()) != 16:
                raise ValueError("each session requires exactly 16 fresh blocks")
        expected = {(task, length) for task in range(4) for length in range(1, 9)}
        if any(len(c) != 32 or set(c) != expected for c in combinations.values()):
            raise ValueError("each block must contain each task/history-length combination once")
    return {"n_rows": len(rows), "sessions": sorted({r["session_id"] for r in rows}),
            "n_blocks": len(block_sessions), "unique_episode_ids": True, "past_only": True}


def load_split(path: Path, expected_split: str, lock_sha256: str, normalization_identity=None,
               require_complete=True):
    rows = [json.loads(line) for line in Path(path).read_text().splitlines() if line.strip()]
    validate_rows(rows, expected_split, lock_sha256, normalization_identity, require_complete)
    return rows


def validate_disjoint(*datasets):
    seen_ids, seen_blocks, seen_sessions, seen_raw = set(), set(), set(), set()
    for rows in datasets:
        ids = {r["episode_id"] for r in rows}
        blocks = {r["block_id"] for r in rows}
        sessions = {r["session_id"] for r in rows}
        raw = {digest for r in rows for digest in r["provenance"].get("raw_record_sha256", [])}
        if ids & seen_ids or blocks & seen_blocks or sessions & seen_sessions or raw & seen_raw:
            raise ValueError("dataset session/block/episode/raw overlap")
        seen_ids |= ids
        seen_blocks |= blocks
        seen_sessions |= sessions
        seen_raw |= raw
    return {"session_disjoint": True, "block_disjoint": True, "episode_disjoint": True,
            "provided_raw_hashes_disjoint": True}


def utilities(row):
    u = 1 - np.minimum(np.asarray(row["costs_seconds"], dtype=float) / row["deadline_seconds"], 2)
    u[np.asarray(row["failures"], dtype=bool)] = -1
    return u


def shuffle_mapping(rows, seed=0):
    """Outcome-independent derangement within session/split/task/history strata."""
    strata = {}
    for i, row in enumerate(rows):
        key = (row["session_id"], row["split"], tuple(row["task_features"]), len(row["body_sequence"]))
        strata.setdefault(key, {}).setdefault(row["block_id"], []).append(i)
    rng = np.random.default_rng(seed + 7103)
    indices = [None] * len(rows)
    for key in sorted(strata):
        groups = strata[key]
        blocks = sorted(groups)
        rng.shuffle(blocks)
        order = [i for block in blocks for i in groups[block]]
        for shift in rng.permutation(np.arange(1, len(order))):
            donors = np.roll(order, int(shift)).tolist()
            if all(rows[a]["block_id"] != rows[b]["block_id"] for a, b in zip(order, donors)):
                for a, b in zip(order, donors):
                    indices[a] = b
                break
        else:
            raise ValueError(f"no legal F2 shuffle derangement in stratum {key}")
    return [{"episode_id": row["episode_id"], "donor_episode_id": rows[j]["episode_id"],
             "recipient_index": i, "donor_index": j, "session_id": row["session_id"],
             "split": row["split"], "task_id": row["task_id"], "history_length": len(row["body_sequence"]),
             "block_id": row["block_id"], "donor_block_id": rows[j]["block_id"], "seed": seed}
            for i, (row, j) in enumerate(zip(rows, indices))]


def body_arrays(row, mode, rng):
    stale = mode in ("STALE", "stale_frames")
    prefix = "stale_" if stale else ""
    v = np.asarray(row[prefix + "body_sequence"], np.float32).copy()
    m = np.asarray(row[prefix + "body_mask_sequence"], np.float32).copy()
    if mode == "BLIND":
        v[:] = 0
        m[:] = 0
    elif mode == "sensor_noise":
        v = np.clip(v + rng.normal(0, .1, v.shape), 0, 1).astype(np.float32)
    elif mode in ("sensor_dropout", "temporal_sensor_dropout"):
        m *= rng.random(m.shape) >= .3
    elif mode == "constant_value":
        v[:] = .5
    elif mode == "scaled_telemetry":
        v = np.clip(v * 1.5, 0, 1)
    elif mode == "partial_sensor_inversion":
        v[:, [1, 5, 9]] = 1 - v[:, [1, 5, 9]]
    elif mode == "sensor_permutation":
        order = np.random.default_rng(557).permutation(20)
        v, m = v[:, order], m[:, order]
    elif mode in ("rtx3060_sensor_unavailable", "controlled_rtx_unavailable"):
        m[:, 4:8] = 0
    elif mode in ("p100_sensor_unavailable", "controlled_p100_unavailable"):
        m[:, 8:12] = 0
    elif mode == "controlled_cpu_constrained":
        m[:, :4] = 0
    elif mode in ("mac_unavailable", "MASTER_ONLY"):
        m[:, 12:16] = 0
    elif mode == "network_latency_increase":
        v[:, 16] = 1
    elif mode == "sampling_interval_2x":
        v, m = v[::-2][::-1].copy(), m[::-2][::-1].copy()
    elif mode == "body_update_delay":
        index = np.maximum(np.arange(len(v)) - 2, 0)
        v, m = v[index], m[index]
    elif mode == "network_jitter":
        for i in range(1, len(v)):
            if rng.random() < .5:
                v[i], m[i] = v[i - 1], m[i - 1]
    elif mode == "task_start_timing":
        keep = int(rng.integers(1, len(v) + 1))
        v, m = v[-keep:], m[-keep:]
    elif mode not in ("BODY", "STALE", "stale_frames"):
        raise ValueError(f"unknown input intervention: {mode}")
    return v, m


def encode_inputs(rows, mode="BODY", seed=0, return_mapping=False):
    mapping = shuffle_mapping(rows, seed) if mode == "SHUFFLED" else None
    tensors = []
    for i, row in enumerate(rows):
        donor = rows[mapping[i]["donor_index"]] if mapping else row
        v, m = body_arrays(donor, "BODY" if mode == "SHUFFLED" else mode,
                           np.random.default_rng(seed * 1000003 + i + 8107))
        task = np.repeat(np.asarray(row["task_features"], np.float32)[None], len(v), 0)
        tensors.append(torch.from_numpy(np.concatenate((task, v * m, m), axis=1)))
    return (tensors, mapping) if return_mapping else tensors
