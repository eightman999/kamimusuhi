"""Versioned, masked alignment of heterogeneous machine telemetry (stdlib only)."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path
import time
from typing import Any

SCHEMA_ID = "k0f.raw.v1"
FRAME_SCHEMA_ID = "k0f.frame.v1"
NORMALIZATION_ID = "k0f.normalize.v1"
FRAME_NAMES = (
    "master_cpu_thermal", "master_cpu_busy", "master_ram_pressure", "master_io_pressure",
    "rtx3060_thermal", "rtx3060_compute_busy", "rtx3060_vram_pressure", "rtx3060_power_pressure",
    "p100_thermal", "p100_compute_busy", "p100_vram_pressure", "p100_power_pressure",
    "mac_thermal", "mac_cpu_busy", "mac_memory_pressure", "mac_power_pressure",
    "network_latency", "network_loss", "body_staleness", "body_availability",
)
DEFAULT_CONFIG = {
    "version": NORMALIZATION_ID, "frame_schema": FRAME_SCHEMA_ID,
    "raw_schema": SCHEMA_ID, "frame_names": list(FRAME_NAMES),
    "temperature_low_c": 30.0, "temperature_high_c": 85.0,
    "rtt_scale_ms": 1000.0, "staleness_scale_s": 60.0, "max_age_s": 60.0,
    "clock_future_tolerance_s": 2.0, "missing_value": 0.0,
    "policy_layout": "20-values-then-20-mask",
}


def canonical_identity(config: dict[str, Any] | None = None) -> str:
    resolved = {**DEFAULT_CONFIG, **(config or {})}
    return hashlib.sha256(json.dumps(resolved, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()).hexdigest()


def _number(value: Any) -> float | None:
    if isinstance(value, bool):
        return float(value)
    if not isinstance(value, (int, float)) or not math.isfinite(value):
        return None
    return float(value)


def _clip(value: float) -> float:
    return max(0.0, min(1.0, value))


def _age(record: dict | None, now: float, config: dict) -> tuple[float | None, dict]:
    if record is None:
        return None, {"available": False}
    source = _number(record.get("timestamp"))
    receipt = _number(record.get("receipt_timestamp"))
    metadata = {key: record.get(key) for key in ("node_id", "sequence", "timestamp", "receipt_timestamp", "source_kind")}
    future_source = source is not None and source > now + config["clock_future_tolerance_s"]
    metadata["source_clock_future"] = future_source
    metadata["receipt_minus_source_s"] = receipt - source if receipt is not None and source is not None else None
    candidates = [now - value for value in (source, receipt) if value is not None and value <= now + config["clock_future_tolerance_s"]]
    age = max(0.0, max(candidates)) if candidates else None
    metadata["age_s"] = age
    metadata["available"] = age is not None and age < config["max_age_s"]
    return age, metadata


def normalize_pair(master: dict | None, mac: dict | None, now: float | None = None, config: dict | None = None) -> dict:
    cfg = {**DEFAULT_CONFIG, **(config or {})}
    if cfg["frame_names"] != list(FRAME_NAMES):
        raise ValueError("frame_names is fixed by k0f.frame.v1")
    if cfg["temperature_high_c"] <= cfg["temperature_low_c"] or min(cfg["rtt_scale_ms"], cfg["staleness_scale_s"], cfg["max_age_s"]) <= 0:
        raise ValueError("normalization scales must be positive")
    for node, record in (("master", master), ("mac", mac)):
        if record is not None and (record.get("schema_version") != SCHEMA_ID or record.get("node_id") != node or record.get("source_kind") not in {"real", "synthetic"}):
            raise ValueError(f"invalid {node} source schema, node or provenance")
    if now is None:
        now = _number((master or {}).get("receipt_timestamp"))
        if now is None:
            now = _number((master or {}).get("timestamp"))
        if now is None:
            now = time.time()
    if _number(now) is None:
        raise ValueError("timestamp must be finite")
    ages: dict[str, float | None] = {}
    provenance: dict[str, Any] = {}
    for node, record in (("master", master), ("mac", mac)):
        ages[node], provenance[node] = _age(record, float(now), cfg)
    values: list[float] = []
    mask: list[float] = []
    quality: list[float] = []
    field_ages: list[float | None] = []

    def field(node: str, keys: tuple[str, ...], transform=lambda x: x[0]):
        record = master if node == "master" else mac
        raw = [_number((record or {}).get("metrics", {}).get(key)) for key in keys]
        q = min((_number((record or {}).get("quality", {}).get(key, 1.0)) or 0.0) for key in keys)
        result = None
        if record and provenance[node]["available"] and all(value is not None for value in raw) and q > 0:
            try:
                result = _number(transform(raw))
            except (ZeroDivisionError, ValueError, OverflowError):
                pass
        valid = result is not None
        values.append(_clip(result) if valid else 0.0)
        mask.append(float(valid))
        quality.append(_clip(q) if valid else 0.0)
        field_ages.append(ages[node])

    thermal = lambda x: (x[0] - cfg["temperature_low_c"]) / (cfg["temperature_high_c"] - cfg["temperature_low_c"])
    ratio = lambda x: x[0] / x[1] if x[1] > 0 and x[0] >= 0 else math.nan
    field("master", ("cpu_temperature_c",), thermal)
    field("master", ("cpu_utilization",))
    field("master", ("memory_pressure",))
    field("master", ("io_pressure",))
    for gpu in ("rtx3060", "p100"):
        field("master", (gpu + "_temperature_c",), thermal)
        field("master", (gpu + "_utilization",))
        field("master", (gpu + "_vram_used_bytes", gpu + "_vram_total_bytes"), ratio)
        field("master", (gpu + "_power_w", gpu + "_power_limit_w"), ratio)
    for metric in ("thermal_pressure", "cpu_utilization", "memory_pressure", "power_pressure"):
        field("mac", (metric,))
    # Prefer peripheral RTT; fall back to the central peer measurement when absent.
    network_source = "mac"
    if (mac is None or not provenance["mac"]["available"] or _number(mac.get("metrics", {}).get("network_loss")) is None):
        network_source = "master"
    field(network_source, ("network_rtt_ms",), lambda x: math.log1p(x[0]) / math.log1p(cfg["rtt_scale_ms"]) if x[0] >= 0 else math.nan)
    field(network_source, ("network_loss",))
    present_ages = [value for value in ages.values() if value is not None]
    stale = max(present_ages) / cfg["staleness_scale_s"] if len(present_ages) == 2 else 1.0
    availability = sum(mask) / len(mask)
    values.extend([_clip(stale), availability])
    mask.extend([1.0, 1.0])
    quality.extend([1.0, 1.0])
    field_ages.extend([0.0, 0.0])
    kinds = {r["source_kind"] for r in (master, mac) if r is not None}
    source_kind = next(iter(kinds)) if len(kinds) == 1 else ("mixed" if kinds else "missing")
    return {
        "schema_version": FRAME_SCHEMA_ID, "normalization_version": NORMALIZATION_ID,
        "normalization_identity": canonical_identity(cfg), "timestamp": float(now),
        "values": values, "mask": mask, "quality": quality, "age_s": field_ages,
        "availability": availability, "source_kind": source_kind, "provenance": provenance,
    }


def policy_input(frame: dict) -> list[float]:
    if frame.get("schema_version") != FRAME_SCHEMA_ID or len(frame.get("values", [])) != 20 or len(frame.get("mask", [])) != 20:
        raise ValueError("policy input requires a versioned 20-value/20-mask frame")
    result = frame["values"] + frame["mask"]
    if any(_number(value) is None or not 0 <= value <= 1 for value in result):
        raise ValueError("policy input values and masks must be finite and bounded")
    if any(value not in (0, 1) for value in frame["mask"]):
        raise ValueError("availability masks must be binary")
    if any(value != 0 for value, valid in zip(frame["values"], frame["mask"]) if not valid):
        raise ValueError("masked policy values must use the configured zero placeholder")
    return [float(value) for value in result]


def align_records(master: dict | None, mac: dict | None, now: float | None = None, config: dict | None = None) -> dict:
    frame = normalize_pair(master, mac, now=now, config=config)
    return {
        "aligned": {"schema_version": "k0f.aligned.v1", "timestamp": frame["timestamp"], "source_kind": frame["source_kind"], "master": master, "mac": mac, "alignment": frame["provenance"]},
        "frame": frame,
        "policy_input": {"schema_version": "k0f.policy-input.v1", "timestamp": frame["timestamp"], "normalization_identity": frame["normalization_identity"], "values": policy_input(frame)},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--master", type=Path, required=True)
    parser.add_argument("--mac", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    masters = [json.loads(line) for line in args.master.read_text().splitlines() if line.strip()]
    macs = sorted((json.loads(line) for line in args.mac.read_text().splitlines() if line.strip()), key=lambda row: row.get("receipt_timestamp", row["timestamp"]))
    args.output.mkdir(parents=True, exist_ok=True)
    streams = {key: (args.output / filename).open("w") for key, filename in (("aligned", "aligned_body_telemetry.jsonl"), ("frame", "interoceptive_frames.jsonl"), ("policy_input", "policy_inputs.jsonl"))}
    index = -1
    try:
        for master in masters:
            now = master.get("receipt_timestamp", master["timestamp"])
            while index + 1 < len(macs) and macs[index + 1].get("receipt_timestamp", macs[index + 1]["timestamp"]) <= now:
                index += 1
            for key, value in align_records(master, macs[index] if index >= 0 else None, now).items():
                streams[key].write(json.dumps(value, allow_nan=False, separators=(",", ":")) + "\n")
    finally:
        for stream in streams.values():
            stream.close()
    (args.output / "normalization_config.json").write_text(json.dumps({**DEFAULT_CONFIG, "identity": canonical_identity()}, indent=2) + "\n")


if __name__ == "__main__":
    main()
