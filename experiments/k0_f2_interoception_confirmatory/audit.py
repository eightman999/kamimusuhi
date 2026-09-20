"""Independent F2 evidence checks, without importing policy/probe metric helpers.

The gate-only path hashes sealed test files but never parses their contents.
Reading those bytes for SHA-256 is deliberately distinct from examining rows.
No experiment is fitted, collected, repaired or promoted by this module.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import hashlib
import json
import math
from pathlib import Path
import re
import statistics as stdstats
import subprocess
from typing import Any

import numpy as np

SCHEMA = "k0-f2-independent-audit-v1"
ROW_SCHEMA = "k0-f2-policy-v1"
FRAME_SCHEMA = "k0f2.frame.v1"
RAW_SCHEMA = "k0f.raw.v1"
MODES = ("BODY", "BLIND", "SHUFFLED", "STALE")
ACTIONS = ("RUN_CPU", "RUN_RTX3060", "RUN_P100", "WAIT")
TASKS = ((128, 8, .003), (512, 16, .008), (1024, 16, .025), (2048, 8, .07))
SESSION_SPLITS = {"A": "train", "B": "train", "C": "validation", "D": "test"}
ATOL = 1e-9


class AuditError(ValueError):
    pass


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with Path(path).open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def canonical_digest(value: Any) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()).hexdigest()


def require(condition: bool, message: str):
    if not condition:
        raise AuditError(message)


def finite(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def close(a, b, tolerance=ATOL) -> bool:
    return bool(np.allclose(a, b, atol=tolerance, rtol=tolerance, equal_nan=False))


def action_utilities(row: dict) -> list[float]:
    costs, failures, deadline = row["costs_seconds"], row["failures"], row["deadline_seconds"]
    require(len(costs) == len(failures) == 4, "exactly four action outcomes required")
    require(finite(deadline) and deadline > 0, "invalid deadline")
    require(all(finite(v) and v >= 0 for v in costs), "invalid cost")
    require(all(type(v) is bool for v in failures), "failure labels must be JSON booleans")
    return [-1.0 if failed else 1.0 - min(cost / deadline, 2.0) for cost, failed in zip(costs, failures)]


def independent_describe(values, *, seed=1847, resamples=20000) -> dict:
    data = [float(v) for v in values]
    require(bool(data) and all(math.isfinite(v) for v in data), "nonempty finite independent seed values required")
    rng = np.random.default_rng(seed)
    array = np.asarray(data, dtype=np.float64)
    draws = array[rng.integers(0, len(array), size=(resamples, len(array)))].mean(axis=1)
    return {"n_seeds": len(data), "mean": stdstats.mean(data),
            "sd": stdstats.stdev(data) if len(data) > 1 else None,
            "median": stdstats.median(data), "ci95": np.quantile(draws, [.025, .975]).tolist(),
            "values": data, "bootstrap_seed": seed, "bootstrap_resamples": resamples,
            "unit": "training_seed"}


def independent_sign_test(differences, tolerance=1e-12) -> dict:
    data = [float(v) for v in differences]
    require(all(math.isfinite(v) for v in data), "nonfinite sign-test input")
    positive = sum(v > tolerance for v in data)
    negative = sum(v < -tolerance for v in data)
    n = positive + negative
    tail = min(positive, negative)
    probability = min(1.0, 2.0 * sum(math.comb(n, i) for i in range(tail + 1)) / (2 ** n)) if n else 1.0
    return {"exact_sign_p": probability, "positive_seeds": positive,
            "negative_seeds": negative, "non_tied_seeds": n, "tied_seeds": len(data) - n}


def independent_pair(left: dict[int, float], right: dict[int, float], *, required_seeds=12,
                     minimum_effect=.01, bootstrap_seed=1847, resamples=20000) -> dict:
    require(set(left) == set(right), "paired seed sets differ")
    seeds = sorted(left)
    require(bool(seeds), "empty seed comparison")
    difference = [left[s] - right[s] for s in seeds]
    result = independent_describe(difference, seed=bootstrap_seed, resamples=resamples)
    result.update(independent_sign_test(difference))
    result.update(seeds=seeds, mean_difference=result["mean"], minimum_effect=minimum_effect,
                  required_seeds=required_seeds,
                  **{"pass": len(seeds) == required_seeds and result["mean"] >= minimum_effect
                     and result["mean"] > 0 and result["ci95"][0] > 0 and result["exact_sign_p"] <= .05})
    return result


def unique_seed_values(rows: list[dict], metric="utility") -> dict[int, float]:
    result = {}
    for row in rows:
        seed = row["seed"]
        require(type(seed) is int and seed not in result, "duplicate or invalid training seed")
        require(finite(row[metric]), "nonfinite seed metric")
        result[seed] = row[metric]
    return result


def audit_baseline(manifest: dict, repo_root: Path) -> dict:
    missing, changed, counts = [], [], {}
    groups = [("tracked", manifest.get("tracked_sha256", manifest.get("sha256", {})), Path(repo_root))]
    if manifest.get("original_k0f_artifacts_sha256"):
        groups.append(("original_k0f_artifacts", manifest["original_k0f_artifacts_sha256"], Path(manifest["original_k0f_artifacts_root"])))
    for label, files, root in groups:
        counts[label] = len(files)
        for relative, expected in files.items():
            path = root / relative
            if not path.is_file():
                missing.append(f"{label}:{relative}")
            elif digest(path) != expected:
                changed.append(f"{label}:{relative}")
    return {"pass": not missing and not changed, "counts": counts, "missing": missing,
            "changed": changed, "old_contents_used_for": "SHA256 only; no old-data fit or analysis"}


def audit_lock(lock_path: Path, repo_root: Path, receipt: dict | None = None) -> dict:
    lock = json.loads(lock_path.read_text())
    source_commit = lock["source_commit"]
    require(re.fullmatch(r"[0-9a-f]{40}", source_commit) is not None, "protocol source commit is not full SHA")
    discrepancies = []
    for relative, expected in lock["source_files_sha256"].items():
        path = repo_root / relative
        if not path.is_file() or digest(path) != expected:
            discrepancies.append(relative)
        version = subprocess.run(["git", "show", f"{source_commit}:{relative}"], cwd=repo_root, capture_output=True)
        if version.returncode or hashlib.sha256(version.stdout).hexdigest() != expected:
            discrepancies.append("source_commit:" + relative)
    result = {"pass": not discrepancies, "source_commit": source_commit,
              "protocol_lock_sha256": digest(lock_path), "checked_source_files": len(lock["source_files_sha256"]),
              "mismatches": discrepancies}
    if receipt is not None:
        commit = receipt.get("protocol_lock_commit", receipt.get("lock_commit", receipt.get("commit")))
        require(commit is not None, "lock receipt does not identify the committed lock")
        tag = subprocess.run(["git", "rev-parse", "F2_PROTOCOL_LOCK^{commit}"], cwd=repo_root, capture_output=True, text=True)
        require(tag.returncode == 0 and tag.stdout.strip() == commit, "F2_PROTOCOL_LOCK tag differs from receipt")
        relative = lock_path.resolve().relative_to(repo_root.resolve()).as_posix()
        contents = subprocess.run(["git", "show", f"{commit}:{relative}"], cwd=repo_root, capture_output=True)
        require(contents.returncode == 0 and hashlib.sha256(contents.stdout).hexdigest() == digest(lock_path), "committed lock bytes differ")
        if receipt.get("protocol_lock_sha256"):
            require(receipt["protocol_lock_sha256"] == digest(lock_path), "lock receipt hash mismatch")
        result["lock_commit"] = commit
    return result


def audit_sessions(sessions: list[dict], *, lock_sha256: str, locked_at: float,
                   buffer_seconds=35, complete=True) -> dict:
    """Metadata/hashes only: safe while session D task rows remain sealed.

    Entry: session_id, split, start_timestamp, end_timestamp, protocol_lock_sha256,
    files={relative:sha256}; optional raw_source_start/end_timestamp.
    """
    seen, raw_hashes, ranges = set(), {}, []
    for item in sessions:
        sid = item["session_id"]
        require(sid in SESSION_SPLITS and sid not in seen, "duplicate or unknown collection session")
        require(item["split"] == SESSION_SPLITS[sid], "session assigned to wrong split")
        require(item["protocol_lock_sha256"] == lock_sha256, "session protocol identity mismatch")
        begin, end = item["start_timestamp"], item["end_timestamp"]
        require(finite(begin) and finite(end) and locked_at < begin < end, "session is not fresh after lock")
        for field in ("raw_source_start_timestamp", "raw_source_end_timestamp"):
            if field in item:
                require(finite(item[field]) and item[field] > locked_at, "source sensor time predates protocol lock")
        seen.add(sid)
        ranges.append((begin, end, sid, item["split"]))
        files = dict(item.get("files", {}))
        files.update({k: item[k] for k in ("raw_mac_sha256", "raw_master_sha256") if k in item})
        for name, value in files.items():
            if "raw_" not in name:
                continue
            require(re.fullmatch(r"[0-9a-f]{64}", value) is not None, "invalid raw SHA256")
            require(value not in raw_hashes, "raw telemetry file content reused across sessions")
            raw_hashes[value] = sid
    if complete:
        require(seen == set(SESSION_SPLITS), "four independent collection sessions required")
    ranges.sort()
    gaps = []
    for first, second in zip(ranges, ranges[1:]):
        gap = second[0] - first[1]
        require(gap >= buffer_seconds, "sessions overlap or lack the locked temporal buffer")
        gaps.append(gap)
    return {"pass": True, "sessions": sorted(seen), "raw_hashes_unique": len(raw_hashes),
            "session_gaps_seconds": gaps, "test_values_opened": False,
            "scope": "collection metadata and byte hashes; source detail checked only for opened splits"}


def audit_rows(rows: list[dict], *, lock_sha256: str, normalization_identity: str,
               frame_index: dict[str, dict] | None = None, allowed_splits=("train", "validation"),
               expected_decisions_per_block=32, require_measurements=True) -> dict:
    seen, blocks, frame_owners = set(), defaultdict(list), {}
    full_validation_seconds = []
    for row in rows:
        require(row.get("schema_version") == ROW_SCHEMA, "invalid F2 row schema")
        require(row["split"] in allowed_splits, "sealed or unexpected split was opened")
        require(row["session_id"] in SESSION_SPLITS and SESSION_SPLITS[row["session_id"]] == row["split"], "row session/split mismatch")
        require(row["protocol_lock_sha256"] == lock_sha256, "row lock mismatch")
        require(row["episode_id"] not in seen, "duplicate episode")
        seen.add(row["episode_id"])
        blocks[row["block_id"]].append(row)
        require(np.asarray(row["task_features"]).shape == (4,) and all(finite(v) for v in row["task_features"]), "invalid task feature vector")
        require(type(row["task_id"]) is int and row["task_id"] in range(4), "unknown task")
        n, repetitions, deadline = TASKS[row["task_id"]]
        expected_task = [n / 2048, repetitions / 16, deadline / .1, 3 * n * n * 4 / (64 * 1024 ** 2)]
        require(close(row["task_features"], expected_task, tolerance=1e-7) and row["deadline_seconds"] == deadline, "task features contain fields beyond locked physical task")
        require(row["provenance"].get("normalization_identity") == normalization_identity, "row normalization identity differs")
        require(row["provenance"].get("telemetry_kind") == "real", "synthetic smoke reused as real collection")
        require(len(row["body_sequence"]) == len(row["stale_body_sequence"]), "STALE changes GRU update count")
        require(finite(row["timestamp"]), "invalid decision timestamp")
        for key, mask_key, reference in (("body_sequence", "body_mask_sequence", "frame_ids"), ("stale_body_sequence", "stale_body_mask_sequence", "stale_frame_ids")):
            values, masks = np.asarray(row[key]), np.asarray(row[mask_key])
            require(values.ndim == 2 and values.shape[1] == 20 and 1 <= len(values) <= 8 and values.shape == masks.shape, "invalid body history shape")
            require(np.isfinite(values).all() and ((values >= 0) & (values <= 1)).all(), "unbounded body values")
            require(np.isin(masks, [0, 1]).all(), "nonbinary missing mask")
            require((values[masks == 0] == 0).all(), "missing value has no neutral placeholder")
            ids = row["provenance"][reference]
            require(len(ids) == len(values), "body history/reference count differs")
            for index, fid in enumerate(ids):
                owner = (row["session_id"], row["split"])
                if fid in frame_owners:
                    require(frame_owners[fid] == owner, "telemetry frame overlaps sessions/splits")
                frame_owners[fid] = owner
                if frame_index is None:
                    continue
                require(fid in frame_index, "referenced telemetry frame missing")
                frame = frame_index[fid]
                require(frame["schema_version"] == FRAME_SCHEMA and frame["source_kind"] == "real", "synthetic or wrong-schema frame in real dataset")
                require(frame["normalization_identity"] == normalization_identity, "frame normalization mismatch")
                limit = row["timestamp"] - (30 if reference == "stale_frame_ids" else 0)
                require(frame["timestamp"] <= limit + 1e-6, "future/stale timestamp leakage")
                if frame.get("session_id"):
                    require(frame["session_id"] == row["session_id"], "source frame from another session")
                expected = list(frame["values"])
                if reference == "stale_frame_ids":
                    expected[18] = min(1., expected[18] + max(0., row["timestamp"] - frame["timestamp"]) / 60.)
                require(close(values[index], expected) and close(masks[index], frame["mask"]), "row does not reconstruct from referenced raw-derived frame")
        action_utilities(row)
        if require_measurements:
            measurements = row["measurements"]
            require(len(measurements) == 4 and sorted(m["action"] for m in measurements) == [0, 1, 2, 3], "action measurement coverage incomplete")
            order = row["action_order"]
            require(sorted(order) == [0, 1, 2, 3], "action order is not a permutation")
            by_action = {m["action"]: m for m in measurements}
            previous_completed = None
            for action in order:
                m = by_action[action]
                stamps = [m[k] for k in ("body_timestamp", "decision_timestamp", "dispatch_timestamp", "start_timestamp", "completion_timestamp", "end_timestamp")]
                require(all(finite(v) for v in stamps) and all(a <= b + 1e-6 for a, b in zip(stamps, stamps[1:])), "job timestamp causality violated")
                if previous_completed is not None:
                    require(previous_completed <= m["dispatch_timestamp"] + 1e-6, "sequential action measurements overlap")
                previous_completed = m["end_timestamp"]
                require(close(m["decision_timestamp"], row["timestamp"]), "measurement decision timestamp differs from base group")
                require(close(row["costs_seconds"][action], m["latency_seconds"]), "row action cost differs from measured job")
                require(close(m["latency_seconds"], m["completion_monotonic"] - m["dispatch_monotonic"], tolerance=3e-6), "compute-completion latency excludes dispatch or WAIT")
                require(close(m["latency_seconds"], m["completion_timestamp"] - m["dispatch_timestamp"], tolerance=1e-4), "wall and monotonic compute-completion time disagree")
                require(m["validation_scope"] == "entire_output_matrix_finite_and_first_row_checksum", "only partial output validation recorded")
                require(finite(m["validation_seconds"]) and m["validation_seconds"] >= 0, "missing full-output validation overhead")
                full_validation_seconds.append(m["validation_seconds"])
                require(row["failures"][action] == (not m.get("ok", True) or not m["finite"]), "failure label disagrees with full output validation")
                if action == 3:
                    require(close(m["wait_seconds"], .020) and m["actual_resource"] in (1, "RUN_RTX3060", "RTX3060", "rtx3060", "cuda:0"), "WAIT semantics differ from locked 20 ms then RTX")
                    require(m["latency_seconds"] >= .020 - 1e-6, "WAIT cost omitted waiting")
                else:
                    require(close(m["wait_seconds"], 0), "non-WAIT action includes an undocumented wait")
    for block, entries in blocks.items():
        require(len({(r["session_id"], r["split"]) for r in entries}) == 1, "block crosses session/split")
        if expected_decisions_per_block is not None:
            require(len(entries) == expected_decisions_per_block, "unexpected decisions per block")
            require(Counter((r["task_id"], len(r["body_sequence"])) for r in entries) == Counter((task, length) for task in range(4) for length in range(1, 9)), "task/history balance differs from lock")
    return {"pass": True, "episodes": len(rows), "blocks": len(blocks), "unique_source_frames": len(frame_owners),
            "opened_splits": sorted({r["split"] for r in rows}), "full_validation_seconds_max": max(full_validation_seconds, default=None),
            "full_validation_seconds_mean": stdstats.mean(full_validation_seconds) if full_validation_seconds else None}


def audit_shuffle(rows: list[dict], mapping: dict[str, str]) -> dict:
    indexed = {r["episode_id"]: r for r in rows}
    require(set(mapping) == set(indexed) and set(mapping.values()) == set(indexed), "SHUFFLED is not a full bijection")
    for recipient, donor in mapping.items():
        a, b = indexed[recipient], indexed[donor]
        require(recipient != donor and a["block_id"] != b["block_id"], "shuffle self/same-block mapping")
        for field in ("split", "session_id", "task_id", "task_features"):
            require(a[field] == b[field], "shuffle changes task/session/split")
        require(len(a["body_sequence"]) == len(b["body_sequence"]), "shuffle changes GRU update count")
    return {"pass": True, "mappings": len(mapping), "same_task_session_length": True, "body_marginal_preserved": True}


def expected_policy_sequence(row: dict, mode="BODY", donor: dict | None = None) -> list[list[float]]:
    source = donor if mode == "SHUFFLED" else row
    require(source is not None, "SHUFFLED input needs an identified donor")
    key = "stale_body_sequence" if mode == "STALE" else "body_sequence"
    mask_key = "stale_body_mask_sequence" if mode == "STALE" else "body_mask_sequence"
    result = []
    for values, masks in zip(source[key], source[mask_key]):
        if mode in ("BLIND", "TRAINED_BLIND"):
            values, masks = [0.] * 20, [0.] * 20
        result.append(list(row["task_features"]) + [v * m for v, m in zip(values, masks)] + list(masks))
    return result


def independent_probe_features(row: dict, mode="BODY", donor: dict | None = None) -> np.ndarray:
    # float32 cast mirrors the recorded model-input boundary, not any fitted transform.
    values = np.asarray(expected_policy_sequence(row, mode, donor), dtype=np.float32).astype(np.float64)
    task = values[-1, :4]
    compute = task[0] ** 3 * task[1]
    body, average = values[-1, 4:], values[:, 4:].mean(axis=0)
    return np.concatenate((task, [task[0] ** 2, compute], body, average,
                           compute * body[:20], compute * average[:20]))


def audit_probe(train: list[dict], validation: list[dict], reported: dict,
                predictions: list[dict], selections: list[dict], *, input_traces=None,
                mapping_records=None, models=None) -> dict:
    """Recompute original targets, all action errors, choices and gate; never fit."""
    require(all(r["split"] == "train" for r in train) and all(r["split"] == "validation" for r in validation), "test data passed to pre-test probe audit")
    lookup = {r["episode_id"]: r for r in train + validation}
    targets = {}
    for split, rows in (("train", train), ("validation", validation)):
        targets[split] = {
            "utility": np.asarray([action_utilities(r) for r in rows]),
            "log_latency": np.asarray([[math.log(min(20., max(1e-8, v))) for v in r["costs_seconds"]] for r in rows]),
        }
    nonconstant = {target: bool(np.std(targets["train"][target]) > 1e-8 and np.std(targets["validation"][target]) > 1e-8)
                   for target in ("utility", "log_latency")}
    validation_ids = {r["episode_id"] for r in validation}
    grouped, indexed_predictions = defaultdict(list), {}
    for p in predictions:
        key = (p["mode"], p["target"], p["episode_id"], p["candidate_action"])
        require(key not in indexed_predictions, "duplicate probe prediction")
        require(p["mode"] in MODES and p["target"] in targets["train"] and p["episode_id"] in validation_ids and p["candidate_action"] in range(4), "probe prediction outside validation design")
        row = lookup[p["episode_id"]]
        actual = action_utilities(row)[p["candidate_action"]] if p["target"] == "utility" else math.log(min(20., max(1e-8, row["costs_seconds"][p["candidate_action"]])))
        require(close(p["observed"], actual), "probe target differs from measured action outcome")
        bounds = (-1., 1.) if p["target"] == "utility" else (math.log(1e-8), math.log(20.))
        require(finite(p["predicted"]) and bounds[0] <= p["predicted"] <= bounds[1], "probe prediction outside fixed target range")
        indexed_predictions[key] = p["predicted"]
        grouped[p["mode"], p["target"]].append(abs(p["predicted"] - actual))
    require(len(indexed_predictions) == len(validation) * 4 * 2 * 4, "probe candidate-action coverage incomplete")
    metrics = {}
    for mode in MODES:
        for target in targets["train"]:
            require(len(grouped[mode, target]) == len(validation) * 4, "probe mode/target coverage incomplete")
            mae = stdstats.mean(grouped[mode, target])
            sd = float(np.std(targets["train"][target]))
            metrics[mode, target] = {"mae": mae, "normalized_mae": mae / sd if sd > 1e-8 else None}
    for summary in reported["rows"]:
        actual = metrics[summary["mode"], summary["target"]]
        require(close(summary["mae"], actual["mae"]), "reported probe MAE differs")
        if actual["normalized_mae"] is not None:
            require(close(summary["normalized_mae"], actual["normalized_mae"]), "probe normalized MAE used nontrain scale")
    regrets, accuracy, selection_seen = defaultdict(list), defaultdict(list), set()
    for selection in selections:
        mode, eid = selection["mode"], selection["episode_id"]
        require((mode, eid) not in selection_seen and mode in MODES and eid in validation_ids, "duplicate/invalid probe choice")
        selection_seen.add((mode, eid))
        predicted = [indexed_predictions[mode, "utility", eid, a] for a in range(4)]
        choice = max(range(4), key=lambda a: predicted[a])
        actual = action_utilities(lookup[eid])
        regret = max(actual) - actual[choice]
        require(selection["selected_action"] == choice and close(selection["predicted_utility"], predicted), "probe action is not fixed-order predicted-utility argmax")
        require(close(selection["actual_utilities"], actual) and close(selection["selection_regret"], regret), "probe selection regret used wrong target")
        require(selection["accuracy"] == (regret <= 1e-12), "probe accuracy tie rule differs")
        regrets[mode].append(regret); accuracy[mode].append(float(regret <= 1e-12))
    require(len(selection_seen) == len(validation) * 4, "probe selection coverage incomplete")
    regret_means = {mode: stdstats.mean(regrets[mode]) for mode in MODES}
    for r in reported["selection_rows"]:
        require(close(r["selection_regret"], regret_means[r["mode"]]) and close(r["accuracy"], stdstats.mean(accuracy[r["mode"]])), "reported selection metrics differ")
    checks = {"both_targets_nonconstant": all(nonconstant.values())}
    for control in ("BLIND", "SHUFFLED"):
        for target in ("log_latency", "utility"):
            checks[f"{target}_BODY_better_than_{control}"] = metrics["BODY", target]["mae"] < metrics[control, target]["mae"]
        checks[f"regret_BODY_better_than_{control}"] = regret_means["BODY"] < regret_means[control]
    require(reported["gate"]["checks"] == checks and reported["gate"]["pass"] == all(checks.values()), "reported validation gate differs from independent calculation")
    verified_inputs = verified_models = 0
    mappings = {r.get("recipient_episode_id", r.get("episode_id")): r.get("donor_episode_id") for r in (mapping_records or [])}
    feature_rows = {}
    if input_traces is not None:
        for trace in input_traces:
            row = lookup[trace["episode_id"]]
            donor = lookup[mappings[row["episode_id"]]] if trace["mode"] == "SHUFFLED" else None
            expected = independent_probe_features(row, trace["mode"], donor)
            require(close(trace["features"], expected, tolerance=2e-7), "probe input contains fields outside the locked 126-feature basis")
            feature_rows[trace["split"], trace["mode"], trace["episode_id"]] = expected
            verified_inputs += 1
        require(verified_inputs == (len(train) + len(validation)) * 4, "probe input evidence coverage incomplete")
    if models is not None:
        require(bool(feature_rows), "model audit requires independently verified input evidence")
        fitted = models.get("models", models)
        for mode in MODES:
            xt = np.stack([feature_rows["train", mode, r["episode_id"]] for r in train])
            xv = np.stack([feature_rows["validation", mode, r["episode_id"]] for r in validation])
            center, scale = xt.mean(axis=0), xt.std(axis=0)
            scale[:6] = np.where(scale[:6] < 1e-8, 1., scale[:6]); scale[6:] = np.maximum(scale[6:], .05)
            for target in ("log_latency", "utility"):
                model = fitted[mode + "/" + target]
                ys = targets["train"][target]
                ycenter, yscale = ys.mean(axis=0), ys.std(axis=0)
                yscale = np.where(yscale < 1e-8, 1., yscale)
                for key, expected in (("x_center", center), ("x_scale", scale), ("y_center", ycenter), ("y_scale", yscale)):
                    require(close(model[key], expected, tolerance=2e-7), "probe normalization was not train-only locked procedure")
                require(model["ridge_lambda"] == 10 and model["scale_floor"] == .05, "probe architecture differs from lock")
                design = np.column_stack((np.ones(len(xv)), (xv - np.asarray(model["x_center"])) / np.asarray(model["x_scale"])))
                output = design @ np.asarray(model["coefficients"]) * np.asarray(model["y_scale"]) + np.asarray(model["y_center"])
                output = np.clip(output, *( (-1, 1) if target == "utility" else (math.log(1e-8), math.log(20)) ))
                saved = np.asarray([[indexed_predictions[mode, target, row["episode_id"], a] for a in range(4)] for row in validation])
                require(close(saved, output, tolerance=2e-7), "saved probe prediction differs from frozen coefficient inference")
                verified_models += 1
    return {"pass": True, "gate_pass": all(checks.values()), "gate_checks": checks,
            "predictions": len(predictions), "selections": len(selections), "verified_input_rows": verified_inputs,
            "verified_frozen_models": verified_models, "fit_performed": False,
            "mae": {mode + "/" + target: value for (mode, target), value in metrics.items()},
            "selection_regret": regret_means, "test_opened": False}


def independent_gru_inference(state: dict, sequence, hidden=None):
    """NumPy implementation of the locked PyTorch GRU equations, no model helper."""
    values = {key: value.detach().cpu().numpy().astype(np.float64) if hasattr(value, "detach") else np.asarray(value, dtype=np.float64) for key, value in state.items()}
    h = np.zeros(values["gru.weight_hh_l0"].shape[1]) if hidden is None else np.asarray(hidden, dtype=np.float64).copy()
    sigmoid = lambda x: 1. / (1. + np.exp(-np.clip(x, -80, 80)))
    for observation in sequence:
        encoded = np.tanh(values["encoder.0.weight"] @ np.asarray(observation) + values["encoder.0.bias"])
        incoming = values["gru.weight_ih_l0"] @ encoded + values["gru.bias_ih_l0"]
        recurrent = values["gru.weight_hh_l0"] @ h + values["gru.bias_hh_l0"]
        ir, iz, inn = np.split(incoming, 3); hr, hz, hn = np.split(recurrent, 3)
        reset, update = sigmoid(ir + hr), sigmoid(iz + hz)
        candidate = np.tanh(inn + reset * hn)
        h = (1. - update) * candidate + update * h
    logits = values["action.weight"] @ h + values["action.bias"]
    require(np.isfinite(logits).all() and np.isfinite(h).all(), "nonfinite independent GRU inference")
    return int(np.argmax(logits)), logits, h


def audit_final_actions(checkpoint_paths: dict[tuple[int, str], Path], traces: list[dict]) -> dict:
    import torch
    states = {key: torch.load(path, map_location="cpu", weights_only=True)["state_dict"] for key, path in checkpoint_paths.items()}
    for trace in traces:
        key = (trace["seed"], trace.get("training_mode", "BLIND" if trace["mode"] == "TRAINED_BLIND" else "BODY"))
        choice, _, _ = independent_gru_inference(states[key], trace["model_input_sequence"])
        actual = ACTIONS.index(trace["action"]) if isinstance(trace["action"], str) else trace["action"]
        require(choice == actual, "saved action differs from independent frozen GRU recomputation")
    return {"pass": True, "actions_recomputed": len(traces), "implementation": "independent NumPy GRU equations"}


def audit_input_traces(rows: list[dict], traces: list[dict], mappings: dict[int, dict[str, str]] | None = None) -> dict:
    indexed = {r["episode_id"]: r for r in rows}
    for trace in traces:
        row, mode = indexed[trace["episode_id"]], trace["mode"]
        donor = indexed[mappings[trace["seed"]][row["episode_id"]]] if mode == "SHUFFLED" else None
        expected = expected_policy_sequence(row, mode, donor)
        require(close(trace["model_input_sequence"], expected, tolerance=2e-7), "actual policy input differs from metadata-free whitelist")
        require(all(len(v) == 44 for v in trace["model_input_sequence"]), "unexpected policy feature dimension")
    return {"pass": True, "traces": len(traces), "input_fields": "task4, masked_body20, mask20; no timestamps/targets/labels"}


def audit_checkpoints(run_paths: list[Path], *, lock_sha256: str, normalization_identity: str, required_seeds=12) -> dict:
    import torch
    groups, entries = {}, []
    for run in run_paths:
        status = json.loads((run / "status.json").read_text())
        require(status.get("complete") is True, "training run incomplete")
        stages = {}
        for stage in ("initial", "best", "final"):
            path = run / f"{stage}.pt"
            checkpoint = torch.load(path, map_location="cpu", weights_only=True)
            meta, state = checkpoint["metadata"], checkpoint["state_dict"]
            require(meta["checkpoint_stage"] == stage, "checkpoint stage mislabeled")
            require(meta["protocol_lock_sha256"] == lock_sha256, "checkpoint protocol identity mismatch")
            require(meta["identities"]["normalization_identity"] == normalization_identity, "checkpoint normalization differs")
            require(all(torch.isfinite(value).all().item() for value in state.values()), "nonfinite checkpoint")
            actual = digest(path)
            require(status["checkpoint_sha256"][stage] == actual, "checkpoint changed after completion")
            require(meta["architecture"] == "GRU128", "architecture differs from confirmatory lock")
            if stage != "initial":
                require(meta["initial_parameter_sha256"] == stages["initial"]["parameter_sha256"], "parent initial parameter identity differs")
            parameter_hash = hashlib.sha256(b"".join(v.detach().cpu().numpy().tobytes() for v in state.values())).hexdigest()
            stages[stage] = {"file_sha256": actual, "parameter_sha256": parameter_hash, "metadata": meta}
        meta = stages["initial"]["metadata"]
        key = (meta["seed"], meta["training_mode"])
        require(key not in groups, "duplicate seed/training-mode run")
        groups[key] = stages
        entries.append({"seed": key[0], "training_mode": key[1], "checkpoint_hashes": {s: v["file_sha256"] for s, v in stages.items()}})
        metrics = [json.loads(line) for line in (run / "metrics.jsonl").open() if line.strip()]
        require(bool(metrics) and all(finite(m["training_loss"]) and finite(m["validation_utility"]) for m in metrics), "missing/nonfinite training metrics")
        best = 0
        for i in range(1, len(metrics)):
            if metrics[i]["validation_utility"] > metrics[best]["validation_utility"] + 1e-12:
                best = i
        require(stages["best"]["metadata"]["best_epoch"] == metrics[best]["epoch"], "checkpoint was not selected by earliest validation maximum")
    seeds = sorted({seed for seed, mode in groups})
    require(seeds == list(range(required_seeds)) and set(groups) == {(seed, mode) for seed in seeds for mode in ("BODY", "BLIND")}, "paired independent BODY/BLIND seed coverage incomplete")
    for seed in seeds:
        require(groups[seed, "BODY"]["initial"]["parameter_sha256"] == groups[seed, "BLIND"]["initial"]["parameter_sha256"], "BODY/BLIND initial weights differ")
    return {"pass": True, "seeds": seeds, "runs": len(run_paths), "checkpoints": 3 * len(run_paths),
            "initial_weights_equal": True, "entries": entries}


def audit_counterfactual(rows: list[dict], pairs: list[dict], *, required_seeds=12) -> dict:
    indexed = {r["episode_id"]: r for r in rows}
    expected = {(a["episode_id"], b["episode_id"]) for a in rows for b in rows
                if a["task_features"] == b["task_features"] and a["block_id"] != b["block_id"]}
    grouped = defaultdict(list)
    for pair in pairs:
        grouped[pair["seed"]].append(pair)
    require(len(grouped) == required_seeds, "counterfactual seed coverage differs")
    matched, frozen = {}, {}
    for seed, entries in grouped.items():
        observed = [(e["source_episode_id"], e["destination_episode_id"]) for e in entries]
        require(len(observed) == len(set(observed)) and set(observed) == expected, "counterfactual omitted or cherry-picked eligible pairs")
        m, f = [], []
        for entry in entries:
            utility = action_utilities(indexed[entry["destination_episode_id"]])
            ma, fa = entry["matched_action"], entry["frozen_action"]
            ma = ACTIONS.index(ma) if isinstance(ma, str) else ma
            fa = ACTIONS.index(fa) if isinstance(fa, str) else fa
            require(close(entry["matched_utility"], utility[ma]) and close(entry["frozen_utility"], utility[fa]), "counterfactual utility is not destination measured outcome")
            m.append(utility[ma]);f.append(utility[fa])
        matched[seed], frozen[seed] = stdstats.mean(m), stdstats.mean(f)
    return {"pass": True, "eligible_pairs_per_seed": len(expected), "seeds": sorted(grouped),
            "comparison": independent_pair(matched, frozen, required_seeds=required_seeds)}


def audit_live(rows: list[dict], *, required_seeds=12, expected_modes=("BODY", "TRAINED_BLIND", "SHUFFLED", "STALE")) -> dict:
    groups, by_mode = defaultdict(list), defaultdict(lambda: defaultdict(list))
    for row in rows:
        require(row["mode"] in expected_modes, "unknown live mode")
        measurement = row["measurement"]
        cost = measurement["latency_seconds"]
        failed = not measurement.get("ok", True) or not measurement["finite"]
        actual = -1. if failed else 1. - min(cost / row["deadline_seconds"], 2.)
        require(close(actual, row["utility"]), "live utility differs from new job latency")
        require(row["timestamp"] <= measurement["dispatch_timestamp"] <= measurement["completion_timestamp"] <= measurement["end_timestamp"], "live timestamps violate causal order")
        require(close(cost, measurement["completion_monotonic"] - measurement["dispatch_monotonic"], tolerance=3e-6), "live latency definition differs from collection")
        require(measurement["validation_scope"] == "entire_output_matrix_finite_and_first_row_checksum", "live job lacked full-output validation")
        groups[row["group_id"]].append(row)
        by_mode[row["mode"]][row["seed"]].append(actual)
    for group, entries in groups.items():
        require(Counter(r["mode"] for r in entries) == Counter(expected_modes), "live group missing/duplicate paired mode")
        require(len({r["seed"] for r in entries}) == 1 and len({r["task_id"] for r in entries}) == 1, "live modes differ in task or seed")
        require(len({r["timestamp"] for r in entries}) == 1, "live modes lack shared base decision timestamp")
    means = {mode: {seed: stdstats.mean(values) for seed, values in seeds.items()} for mode, seeds in by_mode.items()}
    require(all(len(values) == required_seeds for values in means.values()), "live independent seed coverage differs")
    comparison = independent_pair(means["BODY"], means["TRAINED_BLIND"], required_seeds=required_seeds)
    return {"pass": True, "new_jobs": len(rows), "base_groups": len(groups), "seed_means": means,
            "comparison": comparison, "measurement_layer": "new real jobs; no archived action cost used"}


def independent_checksum(matrix_dimension: int, seed_base=20260911) -> float:
    """Small O(n²) reference, no matrix product and no old experiment operands."""
    import torch
    generator = torch.Generator().manual_seed(seed_base + matrix_dimension)
    a = torch.randn((matrix_dimension, matrix_dimension), generator=generator) * .02
    b = torch.randn((matrix_dimension, matrix_dimension), generator=generator) * .02
    return float(np.dot(a[0].numpy().astype(np.float64), b.numpy().astype(np.float64).sum(axis=1)))


def audit_checksums(rows: list[dict], *, seed_base=20260911, atol=1e-4, rtol=1e-4) -> dict:
    references = {task: independent_checksum(TASKS[task][0], seed_base) for task in sorted({r["task_id"] for r in rows})}
    errors = []
    for row in rows:
        expected = references[row["task_id"]]
        for result in row["measurements"]:
            require(finite(result["checksum"]), "missing/nonfinite job checksum")
            error = abs(result["checksum"] - expected)
            require(error <= atol + rtol * abs(expected), "job checksum differs from fresh deterministic independent reference")
            errors.append(error)
    return {"pass": True, "jobs": len(errors), "seed_base": seed_base, "absolute_tolerance": atol,
            "relative_tolerance": rtol, "max_absolute_difference": max(errors, default=0),
            "references": references, "scope": "first-row sum correctness; full-output finite separately verified"}


def audit_collection_telemetry(directory: Path, session_id: str, *, lock: dict) -> tuple[dict, dict]:
    """Inspect only the newly collected session explicitly opened by the caller."""
    raw = {node: read_jsonl(directory / f"raw_{node}_telemetry.jsonl") for node in ("mac", "master")}
    summaries, indexed = {}, {}
    for node, records in raw.items():
        require(len(records) >= 2, "no continuous sensor evidence")
        require(all(r["schema_version"] == RAW_SCHEMA and r["source_kind"] == "real" and r["node_id"] == node for r in records), "synthetic or unknown source in new telemetry")
        times = [r["timestamp"] for r in records]
        require(all(finite(t) and t > lock["locked_at"] for t in times), "source telemetry predates F2 lock")
        require(all(a < b for a, b in zip(times, times[1:])), "sensor source timestamps not strictly increasing")
        sequences = [r["sequence"] for r in records]
        require(len(set(sequences)) == len(sequences), "duplicate raw sensor sequence")
        if node == "mac":
            require(all(finite(r.get("receipt_timestamp")) for r in records), "Mac source/receipt timestamp pair missing")
        indexed[node] = {r["sequence"]: r for r in records}
        metrics = {}
        for name in ("cpu_temperature_c", "rtx3060_temperature_c", "p100_temperature_c", "thermal_pressure", "memory_available_bytes", "daemon_cpu_fraction", "daemon_rss_bytes"):
            values = [r["metrics"].get(name) for r in records if finite(r["metrics"].get(name))]
            if values:
                metrics[name] = {"n": len(values), "missing": len(records)-len(values), "mean": stdstats.mean(values), "min": min(values), "max": max(values)}
        safety = lock["safety"]
        for name, threshold in (("cpu_temperature_c", safety["cpu_stop_c"]), ("rtx3060_temperature_c", safety["gpu_stop_c"]), ("p100_temperature_c", safety["gpu_stop_c"]), ("thermal_pressure", safety["mac_stop_pressure"])):
            if name in metrics:
                require(metrics[name]["max"] < threshold, "recorded thermal safety threshold reached")
        summaries[node] = {"samples": len(records), "source_start_timestamp": times[0], "source_end_timestamp": times[-1],
                           "max_gap_seconds": max(b-a for a,b in zip(times,times[1:])), "sequence_gap_events": sum(b-a != 1 for a,b in zip(sequences,sequences[1:])), "metrics": metrics}
    frames = read_jsonl(directory / "interoceptive_frames.jsonl")
    aligned_path = directory / "aligned_telemetry.jsonl"
    if not aligned_path.exists():
        aligned_path = directory / "aligned_body_telemetry.jsonl"
    aligned = read_jsonl(aligned_path)
    require(len(frames) == len(aligned), "aligned raw/frame counts differ")
    frame_index = {}
    for frame, joined in zip(frames, aligned):
        require(frame["schema_version"] == FRAME_SCHEMA and frame["normalization_identity"] == lock["normalization_identity"], "frame schema or normalization differs from lock")
        require(frame.get("session_id") == session_id and frame["source_kind"] == "real", "frame session identity/synthetic origin invalid")
        require(frame["frame_id"] not in frame_index, "duplicate frame id")
        frame_index[frame["frame_id"]] = frame
        require(len(frame["values"]) == len(frame["mask"]) == 20, "invalid fixed frame shape")
        require(all(finite(v) and 0 <= v <= 1 for v in frame["values"]), "frame has unbounded/nonfinite value")
        require(all(v in (0,1) for v in frame["mask"]), "frame missing mask not binary")
        for node in ("mac", "master"):
            record = joined.get(node)
            if record is not None:
                require(indexed[node].get(record["sequence"]) == record, "aligned record does not match archived raw source")
                require(record["timestamp"] <= frame["timestamp"] + 2, "raw source from future exceeds clock tolerance")
                if node == "mac":
                    require(record["receipt_timestamp"] <= frame["timestamp"] + 1e-6, "Mac value was not received before frame")
    return {"pass": True, "session_id": session_id, "sensors": summaries, "frames": len(frames)}, frame_index


def audit_primary_results(test_rows: list[dict], reported: dict, traces: list[dict]) -> dict:
    indexed = {r["episode_id"]: r for r in test_rows}
    grouped, seen = defaultdict(list), set()
    for trace in traces:
        key = (trace["seed"], trace["training_mode"], trace["mode"], trace["episode_id"])
        require(key not in seen and trace["episode_id"] in indexed, "duplicate/unknown held-out trace")
        seen.add(key)
        row = indexed[trace["episode_id"]]
        action = ACTIONS.index(trace["action"]) if isinstance(trace["action"], str) else trace["action"]
        value = action_utilities(row)[action]
        require(close(trace["utility"], value) and close(trace["latency_seconds"], row["costs_seconds"][action]), "held-out action outcome mismatch")
        grouped[trace["training_mode"], trace["mode"], trace["seed"]].append(value)
    require(len(grouped) == 12 * 5 and all(len(v) == len(test_rows) for v in grouped.values()), "held-out seed/mode/task coverage incomplete")
    means = {key: stdstats.mean(values) for key, values in grouped.items()}
    for row in reported["rows"]:
        key = (row["training_mode"], row["mode"], row["seed"])
        require(close(row["utility"], means[key]), "reported held-out seed utility differs")
    comparisons = {}
    for label, control in (("P1", ("BLIND", "BLIND")), ("P2", ("BODY", "SHUFFLED")), ("P3", ("BODY", "STALE")), ("D_MASKED_BLIND", ("BODY", "BLIND"))):
        actual = independent_pair({s: means["BODY", "BODY", s] for s in range(12)}, {s: means[*control, s] for s in range(12)})
        claimed = reported["comparisons"][label]
        for key in ("mean_difference", "ci95", "exact_sign_p", "values"):
            require(close(actual[key], claimed[key]), "primary paired/sign/bootstrap inputs differ")
        require(actual["pass"] == claimed["pass"], "primary minimum-effect gate differs")
        comparisons[label] = actual
    return {"pass": True, "seed_means_recomputed": len(means), "comparisons": comparisons}


def audit_study(artifacts: Path, lock_path: Path, repo_root: Path, *, include_test=False,
                baseline_path: Path | None = None, lock_receipt: dict | None = None) -> dict:
    """Evidence orchestrator. All test-open guards run before reading test rows."""
    lock = json.loads(lock_path.read_text())
    lock_hash = digest(lock_path)
    gates = {name: None for name in ("fresh_data", "split_disjoint", "normalization_train_only", "leakage_free",
        "checkpoint_identity", "initial_weights_equal", "seed_pairing", "probe_recomputed", "primary_recomputed",
        "live_recomputed", "counterfactual_recomputed", "protocol_lock_valid")}
    evidence = {"schema_version": SCHEMA, "scope": {"gate_only": not include_test, "test_opened": False},
                "gates": gates, "checks": {}, "stage_status": {}, "protocol_lock_sha256": lock_hash}
    if baseline_path is not None:
        evidence["checks"]["immutable_baseline"] = audit_baseline(json.loads(baseline_path.read_text()), repo_root)
    evidence["checks"]["protocol_lock"] = audit_lock(lock_path, repo_root, lock_receipt)
    gates["protocol_lock_valid"] = evidence["checks"]["protocol_lock"]["pass"]
    require(canonical_digest(lock["normalization"]) == lock["normalization_identity"], "normalization constants hash differs from lock")
    gate_path = artifacts / "validation/probe_validation.json"
    probe_report = json.loads(gate_path.read_text()) if gate_path.exists() else None
    freeze_path = artifacts / "train/checkpoint_manifest.json"
    if include_test:
        require(probe_report is not None and probe_report["gate"]["pass"] is True, "test must remain unopened after failed/missing probe")
        require(freeze_path.is_file(), "test requires checkpoint freeze")
        freeze = json.loads(freeze_path.read_text())
        require(freeze.get("complete") is True and freeze["protocol_lock_sha256"] == lock_hash, "test requires completed matching checkpoint freeze")
    collection = json.loads((artifacts / "collection_manifest.json").read_text())
    sessions = collection["sessions"]
    evidence["checks"]["sessions"] = audit_sessions(sessions, lock_sha256=lock_hash, locked_at=lock["locked_at"],
        buffer_seconds=lock["collection"]["inter_session_buffer_seconds"], complete=include_test)
    frame_index, raw_content_hashes = {}, set()
    for session in sessions:
        directory = artifacts / session["relative_path"]
        for key, filename in (("raw_mac_sha256", "raw_mac_telemetry.jsonl"), ("raw_master_sha256", "raw_master_telemetry.jsonl"), ("frames_sha256", "interoceptive_frames.jsonl"), ("dataset_sha256", "dataset.jsonl")):
            require(digest(directory / filename) == session[key], "collection file changed after session manifest")
        if session["split"] == "test" and not include_test:
            continue
        summary, frames = audit_collection_telemetry(directory, session["session_id"], lock=lock)
        require(not set(frames) & set(frame_index), "frame identifiers reused across independent sessions")
        frame_index.update(frames)
        evidence["checks"]["telemetry_" + session["session_id"]] = summary
    gates["fresh_data"] = True
    split_manifest = json.loads((artifacts / "split_manifest.json").read_text())
    splits = {}
    for split in ("train", "validation", "test"):
        entry = split_manifest["splits"].get(split, {})
        if split == "test" and not include_test:
            continue
        if not entry.get("dataset"):
            continue
        path = artifacts / entry["dataset"]
        require(digest(path) == entry["sha256"], "split dataset hash differs")
        splits[split] = read_jsonl(path)
    all_rows = [r for rs in splits.values() for r in rs]
    evidence["checks"]["dataset"] = audit_rows(all_rows, lock_sha256=lock_hash, normalization_identity=lock["normalization_identity"], frame_index=frame_index, allowed_splits=tuple(splits))
    evidence["checks"]["checksums"] = audit_checksums(all_rows, seed_base=lock["matrix_seed_base"],
        atol=lock.get("checksum_tolerance", {}).get("atol", 1e-4), rtol=lock.get("checksum_tolerance", {}).get("rtol", 1e-4))
    gates["split_disjoint"] = True
    gates["normalization_train_only"] = True
    if probe_report is not None:
        directory = artifacts / "validation"
        require(probe_report["protocol_lock_sha256"] == lock_hash, "probe lock differs")
        probe = audit_probe(splits["train"], splits["validation"], probe_report,
            read_jsonl(directory / "probe_predictions.jsonl"), read_jsonl(directory / "probe_selections.jsonl"),
            input_traces=read_jsonl(directory / "probe_inputs.jsonl"), mapping_records=read_jsonl(directory / "probe_shuffle_mapping.jsonl"),
            models=json.loads((directory / "probe_models.json").read_text()))
        evidence["checks"]["probe"] = probe
        gates["probe_recomputed"] = gates["leakage_free"] = True
        if not probe["gate_pass"]:
            require(not freeze_path.exists(), "training freeze exists after failed probe")
            require(not (artifacts / "test/dataset.jsonl").exists(), "test collected after failed probe")
            evidence["study_status"] = "FAIL_AT_PROBE_GATE"
    if freeze_path.exists():
        runs = sorted((artifacts / "train/runs").glob("gru128-s*-*"))
        ck = audit_checkpoints(runs, lock_sha256=lock_hash, normalization_identity=lock["normalization_identity"])
        evidence["checks"]["checkpoints"] = ck
        gates["checkpoint_identity"] = gates["initial_weights_equal"] = gates["seed_pairing"] = True
    if include_test:
        evidence["scope"]["test_opened"] = True
        directory = artifacts / "test"
        report = json.loads((directory / "test_results.json").read_text())
        traces = read_jsonl(directory / "policy_traces.jsonl")
        evidence["checks"]["primary"] = audit_primary_results(splits["test"], report, traces)
        gates["primary_recomputed"] = True
        inputs = read_jsonl(directory / "policy_input_traces.jsonl")
        merged = []
        trace_index = {(r["seed"], r["training_mode"], r["mode"], r["episode_id"]): r for r in traces}
        for record in inputs:
            merged.append({**record, "action": trace_index[record["seed"], record["training_mode"], record["mode"], record["episode_id"]]["action"]})
        paths = {(seed, mode): artifacts / "train/runs" / f"gru128-s{seed}-{mode.lower()}" / "best.pt" for seed in range(12) for mode in ("BODY", "BLIND")}
        evidence["checks"]["final_actions"] = audit_final_actions(paths, merged)
        pair_path = directory / "counterfactual_pairs.jsonl"
        if pair_path.exists():
            evidence["checks"]["counterfactual"] = audit_counterfactual(splits["test"], read_jsonl(pair_path))
            gates["counterfactual_recomputed"] = True
        live_paths = sorted((artifacts / "live").glob("*/live_jobs.jsonl"))
        if live_paths:
            evidence["checks"]["live"] = audit_live([r for p in live_paths for r in read_jsonl(p)])
            gates["live_recomputed"] = True
    for name, state in gates.items():
        evidence["stage_status"][name] = "PASS" if state is True else "FAIL" if state is False else "SKIP_NOT_REACHED"
    evidence["pass"] = all(c.get("pass", True) for c in evidence["checks"].values()) and all(v is not False for v in gates.values())
    evidence["interpretation"] = "Audit PASS verifies reached evidence; it never means research PASS or promotes an unexecuted stage."
    return evidence


def read_jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.open() if line.strip()]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument("--lock", type=Path)
    parser.add_argument("--lock-receipt", type=Path)
    parser.add_argument("--artifacts", type=Path)
    parser.add_argument("--include-test", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.artifacts:
        require(args.lock is not None, "study audit requires a committed protocol lock")
        evidence = audit_study(args.artifacts, args.lock, args.repo_root, include_test=args.include_test,
            baseline_path=args.baseline, lock_receipt=json.loads(args.lock_receipt.read_text()) if args.lock_receipt else None)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        require(not args.output.exists(), "independent audit artifact is immutable")
        args.output.write_text(json.dumps(evidence, indent=2, allow_nan=False) + "\n")
        print(json.dumps({"pass": evidence["pass"], "gates": evidence["gates"]}))
        return
    evidence = {"schema_version": SCHEMA, "scope": {"gate_only": True, "test_opened": False}, "checks": {}}
    if args.baseline:
        evidence["checks"]["immutable_baseline"] = audit_baseline(json.loads(args.baseline.read_text()), args.repo_root)
    if args.lock:
        evidence["checks"]["protocol_lock"] = audit_lock(args.lock, args.repo_root, json.loads(args.lock_receipt.read_text()) if args.lock_receipt else None)
    evidence["pass"] = bool(evidence["checks"]) and all(c["pass"] for c in evidence["checks"].values())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    require(not args.output.exists(), "independent audit artifact is immutable")
    args.output.write_text(json.dumps(evidence, indent=2, allow_nan=False) + "\n")
    print(json.dumps({"pass": evidence["pass"], "checks": sorted(evidence["checks"])}))


if __name__ == "__main__":
    main()
