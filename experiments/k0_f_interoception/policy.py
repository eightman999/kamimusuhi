"""Measured-outcome GRU policy with frozen-input interventions.

Only task features, masked normalized body values and masks enter the model.
Costs, block IDs, workload labels, timestamps and probe targets never enter it.
Counterfactual and OOD results are measured-cost replay, not new physical trials.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
from pathlib import Path
import subprocess
import time

import numpy as np
import torch
from torch import nn
from torch.nn.utils.rnn import pack_padded_sequence, pad_sequence

from .statistics import describe, paired_comparison

SCHEMA = "k0-f-policy-v1"
BODY_DIM = 20
TASK_DIM = 4
ACTION_NAMES = ("RUN_CPU", "RUN_RTX3060", "RUN_P100")
PRIMARY_MODES = ("BODY", "BLIND", "SHUFFLED", "STALE")
SENSOR_OOD = ("sensor_noise", "sensor_dropout", "constant_value", "delayed_telemetry",
              "scaled_telemetry", "rtx3060_sensor_unavailable", "controlled_resource_unavailable", "mac_unavailable",
              "network_latency_increase", "partial_sensor_inversion", "sensor_permutation")
TEMPORAL_OOD = ("sampling_interval_2x", "body_update_delay", "network_jitter",
                "temporal_sensor_dropout", "stale_frames", "task_start_timing")
ANALYSIS_DEFAULTS = {"exploratory_after_failed_probe": False, "confirmatory_eligible": True,
                     "decision_sha256": None, "research_status_locked": None,
                     "failed_probe_sha256": None, "experiment_scope": "confirmatory"}


def analysis_metadata(identities=None):
    result = {key: (identities or {}).get(key, default) for key, default in ANALYSIS_DEFAULTS.items()}
    if result["exploratory_after_failed_probe"]:
        result["experiment_scope"] = "exploratory_after_failed_probe"
        if result["confirmatory_eligible"] is not False or result["research_status_locked"] != "FAIL":
            raise ValueError("diagnostic identities cannot claim confirmatory eligibility or unlock FAIL")
    return result


def admit_policy_run(dataset_path: Path, probe_path: Path, amendment_path: Path | None = None):
    """Keep the normal gate strict; an explicit frozen amendment creates diagnostics."""
    probe = json.loads(probe_path.read_text())
    gate_pass = probe.get("gate", {}).get("pass")
    if gate_pass is not True and gate_pass is not False:
        raise ValueError("probe gate pass must be a JSON boolean")
    if not gate_pass and amendment_path is None:
        raise RuntimeError("validation informativeness gate failed; policy training is not authorized by protocol")
    dataset_hash = sha256(dataset_path)
    if probe.get("dataset_sha256") != dataset_hash:
        raise ValueError("probe gate was not computed on this dataset")
    if amendment_path is None:
        return dict(ANALYSIS_DEFAULTS), None
    if gate_pass:
        raise ValueError("failed-probe diagnostic amendment cannot label a passing probe")
    amendment = json.loads(amendment_path.read_text())
    required = {
        "schema_version": "k0-f-diagnostic-amendment-v1",
        "status": "frozen_before_policy_training_and_test_performance",
        "experiment_scope": "exploratory_after_failed_probe",
        "research_status_locked": "FAIL",
        "dataset_sha256": dataset_hash,
        "probe_sha256": sha256(probe_path),
    }
    for key, expected in required.items():
        if amendment.get(key) != expected:
            raise ValueError(f"diagnostic amendment mismatch: {key}")
    for key in ("confirmatory_eligible", "test_performance_examined_before_amendment"):
        if amendment.get(key) is not False:
            raise ValueError(f"diagnostic amendment requires literal false: {key}")
    return {"exploratory_after_failed_probe": True, "confirmatory_eligible": False,
            "decision_sha256": sha256(amendment_path), "research_status_locked": "FAIL",
            "failed_probe_sha256": sha256(probe_path), "experiment_scope": "exploratory_after_failed_probe"}, amendment


def validate_frozen_diagnostic_policy(amendment, *, seeds, hidden_sizes, epochs, batch_size, learning_rate):
    if amendment is None:
        return
    expected = {"architecture": "GRU128", "seeds": seeds, "epochs": epochs,
                "batch_size": batch_size, "learning_rate": learning_rate,
                "training_modes": ["BODY", "BLIND"], "primary_inputs": list(PRIMARY_MODES),
                "checkpoint_selection": "validation only"}
    if hidden_sizes != [128] or seeds != list(range(8)):
        raise ValueError("diagnostic amendment only permits frozen GRU128 seeds 0..7")
    frozen = amendment.get("frozen_policy", {})
    for key, value in expected.items():
        if frozen.get(key) != value:
            raise ValueError(f"diagnostic policy differs from frozen amendment: {key}")


def json_write(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n")
    temporary.replace(path)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require_finite_tensors(values, description):
    if not all(torch.isfinite(value).all().item() for value in values):
        raise FloatingPointError(f"nonfinite {description}; run stopped without completion")


def unique_integer_list(value: str, name: str, minimum: int = 0):
    parsed = [int(item) for item in value.split(",")]
    if len(set(parsed)) != len(parsed):
        raise ValueError(f"duplicate {name} are not independent experiment units")
    if not parsed or min(parsed) < minimum:
        raise ValueError(f"{name} must be >= {minimum}")
    return parsed


def require_unique_seed_rows(rows):
    keys = [(r["architecture"], r["training_mode"], r["checkpoint"], r["mode"], r["seed"]) for r in rows]
    if len(set(keys)) != len(keys):
        raise ValueError("duplicate architecture/training_mode/checkpoint/mode/seed result rows")


def validate_rows(rows: list[dict], require_all_splits: bool = True) -> dict:
    blocks, ids, split_counts = {}, set(), {}
    for row in rows:
        if row.get("schema_version", SCHEMA) != SCHEMA:
            raise ValueError("unsupported policy schema")
        if row["episode_id"] in ids:
            raise ValueError("duplicate episode_id")
        ids.add(row["episode_id"])
        split = row["split"]
        if split not in ("train", "validation", "test"):
            raise ValueError("invalid split")
        block = row["block_id"]
        if block in blocks and blocks[block] != split:
            raise ValueError("workload block crosses data splits")
        blocks[block] = split
        split_counts[split] = split_counts.get(split, 0) + 1
        task = np.asarray(row["task_features"], dtype=float)
        if task.shape != (TASK_DIM,) or not np.isfinite(task).all():
            raise ValueError("task_features must be four finite values")
        for key, mask_key in (("body_sequence", "body_mask_sequence"),
                              ("stale_body_sequence", "stale_body_mask_sequence")):
            values = np.asarray(row[key], dtype=float)
            masks = np.asarray(row[mask_key], dtype=float)
            if values.ndim != 2 or values.shape[1] != BODY_DIM or not 1 <= len(values) <= 8:
                raise ValueError("body sequence shape must be [1..8,20]")
            if values.shape != masks.shape or not np.isfinite(values).all() or not np.isfinite(masks).all():
                raise ValueError("body masks and values must align and be finite")
            if np.any((values < 0) | (values > 1)) or np.any((masks < 0) | (masks > 1)):
                raise ValueError("body values and masks must be in [0,1]")
        costs = np.asarray(row["costs_seconds"], dtype=float)
        failures = np.asarray(row["failures"], dtype=bool)
        if costs.shape != (3,) or failures.shape != (3,) or not np.isfinite(costs).all() or (costs < 0).any():
            raise ValueError("three measured finite nonnegative action costs are required")
        if not math.isfinite(row["deadline_seconds"]) or row["deadline_seconds"] <= 0:
            raise ValueError("deadline must be positive")
    if require_all_splits and set(split_counts) != {"train", "validation", "test"}:
        raise ValueError("train, validation and test splits are required")
    return {"split_counts": split_counts, "block_counts": {
        s: sum(v == s for v in blocks.values()) for s in split_counts},
        "blocks_disjoint": True, "episodes_unique": True}


def load_dataset(path: Path) -> list[dict]:
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    validate_rows(rows)
    return rows


def utilities(row: dict) -> np.ndarray:
    utility = 1.0 - np.minimum(np.asarray(row["costs_seconds"], dtype=float) / row["deadline_seconds"], 2.0)
    utility[np.asarray(row["failures"], dtype=bool)] = -1.0
    return utility


def _shuffled_indices(rows: list[dict], seed: int) -> list[int]:
    """Length-stratified bijection to different blocks; GRU update count is fixed."""
    rng = np.random.default_rng(seed + 7103)
    # Block-sorted cyclic rotations provide a derangement when arbitrary random
    # permutations would frequently leave same-block body/target correspondence.
    strata = {}
    for i, row in enumerate(rows):
        strata.setdefault(len(row["body_sequence"]), {}).setdefault(row["block_id"], []).append(i)
    result = [0] * len(rows)
    for length in sorted(strata):
        groups = strata[length]
        group_keys = sorted(groups)
        rng.shuffle(group_keys)
        order = [i for key in group_keys for i in groups[key]]
        for shift in rng.permutation(np.arange(1, len(order))):
            donor = np.roll(order, int(shift)).tolist()
            if all(rows[a]["block_id"] != rows[b]["block_id"] for a, b in zip(order, donor)):
                for a, b in zip(order, donor):
                    result[a] = b
                break
        else:
            raise ValueError(f"SHUFFLED has no same-history-length cross-block bijection: length={length}, block_counts={dict((k, len(v)) for k, v in groups.items())}")
    return result


def body_arrays(row: dict, mode: str, rng: np.random.Generator):
    stale = mode in ("STALE", "delayed_telemetry", "stale_frames")
    key = "stale_body_sequence" if stale else "body_sequence"
    mask_key = "stale_body_mask_sequence" if stale else "body_mask_sequence"
    v, m = np.array(row[key], dtype=np.float32), np.array(row[mask_key], dtype=np.float32)
    if mode == "BLIND":
        v[:] = 0
        m[:] = 0
    elif mode == "sensor_noise":
        v = np.clip(v + rng.normal(0, .1, v.shape), 0, 1).astype(np.float32)
    elif mode in ("sensor_dropout", "temporal_sensor_dropout"):
        m *= (rng.random(m.shape) >= .3)
    elif mode == "constant_value":
        v[:] = .5
    elif mode == "scaled_telemetry":
        v = np.clip(v * 1.5, 0, 1)
    elif mode in ("rtx3060_sensor_unavailable", "controlled_resource_unavailable"):
        # Observation outage only: actual resource failure was not induced.
        m[:, 4:8] = 0
    elif mode == "mac_unavailable":
        m[:, 12:16] = 0
    elif mode == "network_latency_increase":
        v[:, 16] = 1
    elif mode == "partial_sensor_inversion":
        v[:, [1, 5, 9]] = 1 - v[:, [1, 5, 9]]
    elif mode == "sensor_permutation":
        permutation = np.random.default_rng(557).permutation(BODY_DIM)
        v, m = v[:, permutation], m[:, permutation]
    elif mode == "sampling_interval_2x":
        v, m = v[::-2][::-1].copy(), m[::-2][::-1].copy()
    elif mode == "body_update_delay":
        # Two observed samples of delay (seconds depend on acquisition cadence).
        positions = np.maximum(np.arange(len(v)) - 2, 0)
        v, m = v[positions], m[positions]
    elif mode == "network_jitter":
        for i in range(1, len(v)):
            if rng.random() < .5:
                v[i], m[i] = v[i - 1], m[i - 1]
    elif mode == "task_start_timing":
        keep = int(rng.integers(1, len(v) + 1))
        v, m = v[-keep:], m[-keep:]
    return v, m


def encode_inputs(rows: list[dict], mode: str, seed: int = 0):
    """Explicit whitelist. Row order/task columns never change across interventions."""
    indices = _shuffled_indices(rows, seed) if mode == "SHUFFLED" else list(range(len(rows)))
    result = []
    for i, row in enumerate(rows):
        donor = rows[indices[i]]
        rng = np.random.default_rng(seed * 1000003 + i + 8107)
        values, masks = body_arrays(donor, "BODY" if mode == "SHUFFLED" else mode, rng)
        tasks = np.repeat(np.asarray(row["task_features"], dtype=np.float32)[None], len(values), axis=0)
        result.append(torch.from_numpy(np.concatenate((tasks, values * masks, masks), axis=1)))
    return result


class BodyPolicy(nn.Module):
    def __init__(self, hidden_size: int = 128):
        super().__init__()
        self.hidden_size = hidden_size
        self.encoder = nn.Sequential(nn.Linear(TASK_DIM + BODY_DIM * 2, hidden_size), nn.Tanh())
        self.gru = nn.GRU(hidden_size, hidden_size, batch_first=True)
        self.action = nn.Linear(hidden_size, len(ACTION_NAMES))

    def forward(self, sequences: list[torch.Tensor], hidden=None):
        lengths = torch.tensor([len(s) for s in sequences], dtype=torch.long)
        packed = pack_padded_sequence(self.encoder(pad_sequence(sequences, batch_first=True)),
                                      lengths.cpu(), batch_first=True, enforce_sorted=False)
        _, state = self.gru(packed, hidden)
        return self.action(state[-1]), state


def infer(model: BodyPolicy, inputs: list[torch.Tensor], batch_size: int = 128):
    device = next(model.parameters()).device
    predictions, norms = [], []
    model.eval()
    with torch.no_grad():
        for start in range(0, len(inputs), batch_size):
            logits, hidden = model([x.to(device) for x in inputs[start:start + batch_size]])
            require_finite_tensors([logits, hidden], "inference outputs")
            predictions.extend(logits.argmax(-1).cpu().tolist())
            norms.extend(hidden[-1].norm(dim=-1).cpu().tolist())
    return predictions, norms


def score_actions(rows: list[dict], actions: list[int]) -> dict:
    if len(rows) != len(actions) or not rows:
        raise ValueError("one action per nonempty evaluation row required")
    latencies = [r["costs_seconds"][a] for r, a in zip(rows, actions)]
    rewards = [float(utilities(r)[a]) for r, a in zip(rows, actions)]
    failures = [r["failures"][a] for r, a in zip(rows, actions)]
    oracle = [int(np.argmax(utilities(r))) for r in rows]
    return {"n_episodes": len(rows), "utility": float(np.mean(rewards)),
            "latency_seconds": float(np.mean(latencies)),
            "deadline_success_rate": float(np.mean([not f and l <= r["deadline_seconds"]
                for r, f, l in zip(rows, failures, latencies)])),
            "failure_rate": float(np.mean(failures)),
            "oracle_match_rate": float(np.mean(np.equal(actions, oracle))),
            "oracle_utility": float(np.mean([max(utilities(r)) for r in rows])),
            "action_rates": {name: actions.count(i) / len(actions) for i, name in enumerate(ACTION_NAMES)}}


def train_one(train_rows, validation_rows, output: Path, *, seed=0, hidden_size=128,
              mode="BODY", epochs=160, batch_size=64, learning_rate=1e-3,
              source_commit="unknown", identities=None, device="cpu"):
    """Validation utility selects immutable best; held-out rows are not accepted."""
    if any(r["split"] != "train" for r in train_rows) or any(r["split"] != "validation" for r in validation_rows):
        raise ValueError("train_one accepts only train and validation rows")
    analysis = analysis_metadata(identities)
    if output.exists() and any(output.iterdir()):
        raise FileExistsError(f"refusing to overwrite existing training run: {output}")
    output.mkdir(parents=True, exist_ok=True)
    torch.manual_seed(seed)
    np.random.seed(seed)
    torch.use_deterministic_algorithms(True)
    model = BodyPolicy(hidden_size).to(device)
    initial_state = {k: v.detach().cpu().clone() for k, v in model.state_dict().items()}
    require_finite_tensors(initial_state.values(), "initial parameters")
    initial_hash = hashlib.sha256(b"".join(v.detach().cpu().numpy().tobytes() for v in model.state_dict().values())).hexdigest()
    optimizer = torch.optim.Adam(model.parameters(), lr=learning_rate)
    inputs = encode_inputs(train_rows, mode, seed)
    val_inputs = encode_inputs(validation_rows, mode, seed)
    utility_matrix = torch.tensor(np.stack([utilities(r) for r in train_rows]), dtype=torch.float32, device=device)
    labels = utility_matrix.argmax(-1)
    # Loss fixed in advance: teacher CE plus expected measured utility regret.
    best, best_epoch, best_state = -math.inf, -1, None
    generator = torch.Generator().manual_seed(seed + 2909)
    metrics, started = [], time.monotonic()
    for epoch in range(epochs):
        model.train()
        order = torch.randperm(len(train_rows), generator=generator).tolist()
        losses = []
        for start in range(0, len(order), batch_size):
            idx = order[start:start + batch_size]
            logits, _ = model([inputs[i].to(device) for i in idx])
            expected_regret = (utility_matrix[idx].max(-1).values - (logits.softmax(-1) * utility_matrix[idx]).sum(-1)).mean()
            loss = nn.functional.cross_entropy(logits, labels[idx]) + expected_regret
            require_finite_tensors([loss], "training loss")
            optimizer.zero_grad(set_to_none=True)
            loss.backward()
            require_finite_tensors([p.grad for p in model.parameters() if p.grad is not None], "training gradients")
            nn.utils.clip_grad_norm_(model.parameters(), 1.0, error_if_nonfinite=True)
            optimizer.step()
            require_finite_tensors(model.state_dict().values(), "updated parameters")
            losses.append(float(loss.detach().cpu()))
        actions, _ = infer(model, val_inputs)
        validation = score_actions(validation_rows, actions)
        if validation["utility"] > best + 1e-12:
            best, best_epoch = validation["utility"], epoch
            best_state = {k: v.detach().cpu().clone() for k, v in model.state_dict().items()}
        entry = {**analysis, "epoch": epoch, "training_loss": float(np.mean(losses)),
                 "validation_utility": validation["utility"], "validation_latency_seconds": validation["latency_seconds"]}
        metrics.append(entry)
        with (output / "metrics.jsonl").open("a") as handle:
            handle.write(json.dumps(entry) + "\n")
    if best_state is None:
        raise ValueError("epochs must be positive")
    final_state = {k: v.detach().cpu().clone() for k, v in model.state_dict().items()}
    for stage, state in (("initial", initial_state), ("best", best_state), ("final", final_state)):
        require_finite_tensors(state.values(), f"{stage} checkpoint parameters")
    metadata = {**analysis, "schema_version": SCHEMA, "architecture": f"GRU{hidden_size}", "seed": seed,
                "parent": None, "training_stage": "supervised_measured_cost_imitation",
                "training_mode": mode, "source_commit": source_commit,
                "identities": identities or {}, "initial_parameter_sha256": initial_hash,
                "selection": "maximum validation mean utility; earliest strict improvement", "best_epoch": best_epoch,
                "best_validation_utility": best, "final_validation_utility": metrics[-1]["validation_utility"],
                "epochs": epochs, "learning_rate": learning_rate, "batch_size": batch_size,
                "loss": "teacher cross_entropy + expected measured utility regret",
                "elapsed_seconds": time.monotonic() - started,
                "train_episode_ids": [r["episode_id"] for r in train_rows],
                "validation_episode_ids": [r["episode_id"] for r in validation_rows]}
    for stage, state in (("best", best_state), ("final", final_state)):
        torch.save({"metadata": dict(metadata, checkpoint_stage=stage), "state_dict": state}, output / f"{stage}.pt")
    latency_model = copy.deepcopy(model).cpu().eval()
    latency_input = [val_inputs[0].cpu()]
    measurements = []
    with torch.no_grad():
        for _ in range(3):
            latency_model(latency_input)
        for _ in range(30):
            tick = time.perf_counter()
            latency_model(latency_input)
            measurements.append((time.perf_counter() - tick) * 1000)
    metadata["parameter_count"] = sum(p.numel() for p in model.parameters())
    metadata["cpu_inference_ms"] = float(np.median(measurements))
    metadata["cpu_inference_protocol"] = {"batch_size": 1, "sequence_length": len(val_inputs[0]), "warmups": 3, "repeats": 30, "torch_threads": torch.get_num_threads(), "checkpoint_stage": "final"}
    metadata["checkpoint_sha256"] = {stage: sha256(output / f"{stage}.pt") for stage in ("best", "final")}
    json_write(output / "status.json", dict(metadata, complete=True, finite_parameters=True,
        finite_checkpoints={"initial": True, "best": True, "final": True}, finite_loss_and_gradients_every_update=True))
    return metadata


def load_model(path: Path, device="cpu"):
    checkpoint = torch.load(path, map_location="cpu", weights_only=True)
    metadata = checkpoint["metadata"]
    model = BodyPolicy(int(metadata["architecture"].replace("GRU", ""))).to(device)
    model.load_state_dict(checkpoint["state_dict"])
    model.eval()
    return model, metadata


def counterfactual(model: BodyPolicy, rows: list[dict], seed: int) -> dict:
    """A/B final frame only, identical h from A history; B measured-cost outcome.

    All same-task cross-block ordered pairs are used, without selecting pairs by
    oracle action, observed reward, workload label, or model result.
    """
    groups = {}
    for i, row in enumerate(rows):
        groups.setdefault(tuple(row["task_features"]), []).append(i)
    inputs = encode_inputs(rows, "BODY", seed)
    matched, frozen, destinations, details = [], [], [], []
    device = next(model.parameters()).device
    with torch.no_grad():
        for indices in groups.values():
            for ai in indices:
                a = inputs[ai].to(device)
                hidden = None
                if len(a) > 1:
                    _, hidden = model([a[:-1]])
                action_a = int(model([a[-1:]], hidden)[0].argmax(-1).item())
                for bi in indices:
                    if rows[ai]["block_id"] == rows[bi]["block_id"]:
                        continue
                    b_final = inputs[bi][-1:].to(device)
                    action_b = int(model([b_final], hidden)[0].argmax(-1).item())
                    matched.append(action_b)
                    frozen.append(action_a)
                    destinations.append(rows[bi])
                    details.append({"source_episode_id": rows[ai]["episode_id"],
                        "destination_episode_id": rows[bi]["episode_id"],
                        "frozen_action": ACTION_NAMES[action_a], "matched_action": ACTION_NAMES[action_b],
                        "frozen_utility": float(utilities(rows[bi])[action_a]),
                        "matched_utility": float(utilities(rows[bi])[action_b])})
    if not destinations:
        return {"seed": seed, "n_pairs": 0, "available": False, "reason": "no repeated exact tasks across blocks"}, []
    ms, fs = score_actions(destinations, matched), score_actions(destinations, frozen)
    return {"seed": seed, "architecture": f"GRU{model.hidden_size}", "available": True,
            "n_pairs": len(destinations), "action_change_rate": float(np.mean(np.not_equal(matched, frozen))),
            "matched_utility": ms["utility"], "frozen_utility": fs["utility"],
            "utility_gain": ms["utility"] - fs["utility"],
            "matched_latency_seconds": ms["latency_seconds"], "frozen_latency_seconds": fs["latency_seconds"],
            "outcome_provenance": "measured_action_cost_replay",
            "control": "identical task, A history, model weights and hidden; replace final body only"}, details


def evaluate_models(rows, run_paths, output: Path):
    test = [r for r in rows if r["split"] == "test"]
    if len(set(Path(p).resolve() for p in run_paths)) != len(run_paths):
        raise ValueError("duplicate run paths are not independent seeds")
    ablations, ood, counter, traces, forks, input_traces, final_results = [], [], [], [], [], [], []
    run_identities = set()
    analysis = None
    for run in run_paths:
        model, meta = load_model(run / "best.pt")
        run_analysis = analysis_metadata(meta["identities"])
        if analysis is not None and run_analysis != analysis:
            raise ValueError("cannot mix confirmatory and diagnostic runs or different amendments")
        analysis = run_analysis
        seed, training_mode = meta["seed"], meta["training_mode"]
        run_identity = (meta["architecture"], training_mode, seed)
        if run_identity in run_identities:
            raise ValueError("duplicate architecture/training_mode/seed checkpoints")
        run_identities.add(run_identity)
        modes = PRIMARY_MODES if training_mode == "BODY" else ("BLIND",)
        final_model, final_meta = load_model(run / "final.pt")
        if final_meta["checkpoint_stage"] != "final" or final_meta["seed"] != seed or final_meta["training_mode"] != training_mode:
            raise ValueError("final checkpoint stage/seed/mode mismatch")
        for mode in modes:
            encoded = encode_inputs(test, mode, seed)
            actions, norms = infer(model, encoded)
            final_actions, _ = infer(final_model, encoded)
            final_results.append(dict(seed=seed, architecture=meta["architecture"], training_mode=training_mode,
                checkpoint="final", mode=mode, checkpoint_sha256=sha256(run / "final.pt"), **score_actions(test, final_actions)))
            ablations.append(dict(seed=seed, architecture=meta["architecture"], training_mode=training_mode,
                checkpoint="best", mode=mode, **score_actions(test, actions)))
            for row, action, norm, sequence in zip(test, actions, norms, encoded):
                input_traces.append({"seed": seed, "architecture": meta["architecture"], "training_mode": training_mode,
                    "mode": mode, "episode_id": row["episode_id"], "model_input_sequence": sequence.tolist(),
                    "layout": "task4, masked_body20, mask20", "input_dimension": TASK_DIM + BODY_DIM * 2,
                    "source_kind": "real_telemetry_with_prespecified_input_intervention"})
                traces.append({"seed": seed, "architecture": meta["architecture"], "training_mode": training_mode,
                    "mode": mode, "episode_id": row["episode_id"], "block_id": row["block_id"],
                    "action": ACTION_NAMES[action], "hidden_norm": norm,
                    "source_kind": "real_telemetry_measured_cost_replay",
                    "timestamp": row.get("timestamp", row.get("provenance", {}).get("timestamp")),
                    "body_values": row["body_sequence"][-1], "body_mask": row["body_mask_sequence"][-1],
                    "utility": float(utilities(row)[action]), "latency_seconds": row["costs_seconds"][action]})
        if training_mode == "BODY":
            result, detail = counterfactual(model, test, seed)
            counter.append(result)
            forks.extend(dict(d, seed=seed, architecture=meta["architecture"]) for d in detail)
            for mode in TEMPORAL_OOD + SENSOR_OOD:
                actions, _ = infer(model, encode_inputs(test, mode, seed))
                outcomes = test
                if mode == "controlled_resource_unavailable":
                    outcomes = copy.deepcopy(test)
                    for row in outcomes:
                        row["failures"][1] = True
                ood.append(dict(seed=seed, architecture=meta["architecture"], mode=mode,
                    category="temporal" if mode in TEMPORAL_OOD else "sensor",
                    kind="synthetic_resource_failure_simulation" if mode == "controlled_resource_unavailable" else "synthetic_observation_perturbation",
                    outcome_provenance="RTX_selection_failure_simulated; other_actions_measured_cost" if mode == "controlled_resource_unavailable" else "measured_action_cost_replay",
                    **score_actions(outcomes, actions)))
    if analysis is None:
        raise ValueError("at least one run is required")
    for records in (ablations, ood, counter, traces, forks, input_traces, final_results):
        for record in records:
            record.update(analysis)
    require_unique_seed_rows(ablations)
    require_unique_seed_rows(final_results)
    primary = [r for r in ablations if r["training_mode"] == "BODY"]
    comparisons, aggregates = {}, {}
    for architecture in sorted(set(r["architecture"] for r in primary)):
        table = {mode: sorted([r for r in primary if r["architecture"] == architecture and r["mode"] == mode], key=lambda r: r["seed"]) for mode in PRIMARY_MODES}
        for mode, entries in table.items():
            aggregates[f"{architecture}_{mode}"] = {metric: describe([r[metric] for r in entries]) for metric in
                ("utility", "latency_seconds", "deadline_success_rate", "failure_rate", "oracle_match_rate")}
        for mode in PRIMARY_MODES[1:]:
            if [r["seed"] for r in table["BODY"]] != [r["seed"] for r in table[mode]]:
                raise ValueError("unpaired seed sets")
            comparisons[f"{architecture}_BODY_vs_{mode}"] = paired_comparison([r["utility"] for r in table["BODY"]], [r["utility"] for r in table[mode]])
        independent = sorted([r for r in ablations if r["architecture"] == architecture and r["training_mode"] == "BLIND"], key=lambda r: r["seed"])
        if independent and [r["seed"] for r in independent] == [r["seed"] for r in table["BODY"]]:
            comparisons[f"{architecture}_BODY_vs_independently_trained_BLIND"] = paired_comparison([r["utility"] for r in table["BODY"]], [r["utility"] for r in independent])
    json_write(output / "ablation_results.json", {**analysis, "schema_version": SCHEMA, "rows": ablations, "comparisons": comparisons, "statistics": aggregates,
        "primary_control": "same BODY checkpoint; independent BLIND training is secondary", "outcome_provenance": "measured_action_cost_replay"})
    final_statistics, final_vs_best = {}, {}
    for architecture, training_mode, mode in sorted(set((r["architecture"], r["training_mode"], r["mode"]) for r in final_results)):
        final_rows = sorted([r for r in final_results if (r["architecture"], r["training_mode"], r["mode"]) == (architecture, training_mode, mode)], key=lambda r: r["seed"])
        best_rows = sorted([r for r in ablations if (r["architecture"], r["training_mode"], r["mode"]) == (architecture, training_mode, mode)], key=lambda r: r["seed"])
        if [r["seed"] for r in final_rows] != [r["seed"] for r in best_rows]:
            raise ValueError("best/final comparison seed mismatch")
        key = f"{architecture}_trained_{training_mode}_input_{mode}"
        final_statistics[key] = {metric: describe([r[metric] for r in final_rows]) for metric in
            ("utility", "latency_seconds", "deadline_success_rate", "failure_rate", "oracle_match_rate")}
        final_vs_best[key] = paired_comparison([r["utility"] for r in final_rows], [r["utility"] for r in best_rows])
    json_write(output / "final_checkpoint_results.json", {**analysis, "schema_version": SCHEMA, "rows": final_results,
        "statistics": final_statistics, "final_vs_best_utility": final_vs_best,
        "checkpoint": "final", "primary": False, "outcome_provenance": "measured_action_cost_replay",
        "selection_note": "Final checkpoint is reported separately; validation-best remains the frozen primary choice."})
    counter_stats = {arch: paired_comparison([r["matched_utility"] for r in counter if r.get("available") and r["architecture"] == arch],
        [r["frozen_utility"] for r in counter if r.get("available") and r["architecture"] == arch]) for arch in sorted(set(r["architecture"] for r in counter if r.get("available")))}
    counter_descriptive = {arch: {metric: describe([r[metric] for r in counter if r.get("available") and r["architecture"] == arch])
        for metric in ("action_change_rate", "matched_utility", "frozen_utility", "utility_gain", "matched_latency_seconds", "frozen_latency_seconds")}
        for arch in counter_stats}
    json_write(output / "counterfactual_body.json", {**analysis, "schema_version": SCHEMA, "rows": counter, "comparisons": counter_stats, "statistics": counter_descriptive,
        "limitation": "Archived real outcomes; body swapping is controlled in model replay, not a randomized physical intervention."})
    ood_statistics = {f"{arch}_{mode}": {metric: describe([r[metric] for r in ood if r["architecture"] == arch and r["mode"] == mode])
        for metric in ("utility", "latency_seconds", "deadline_success_rate", "failure_rate", "oracle_match_rate")}
        for arch in sorted(set(r["architecture"] for r in ood)) for mode in TEMPORAL_OOD + SENSOR_OOD}
    json_write(output / "ood_results.json", {**analysis, "schema_version": SCHEMA, "rows": ood, "statistics": ood_statistics,
        "limitation": "Synthetic observation changes scored against fixed measured costs; no real network fault, GPU failure, or unsafe workload induced.",
        "rtx3060_sensor_unavailable_scope": "RTX3060 sensor channels missing; measured costs unchanged",
        "controlled_resource_unavailable_scope": "RTX3060 sensor channels missing; selecting RTX receives utility=-1 and failure=1; physical GPU remains available"})
    for filename, entries in (("policy_traces.jsonl", traces), ("counterfactual_pairs.jsonl", forks), ("policy_input_traces.jsonl", input_traces)):
        (output / filename).write_text("".join(json.dumps(r, allow_nan=False) + "\n" for r in entries))


def verified_completed_run(run: Path, identities: dict):
    status = json.loads((run / "status.json").read_text())
    if not status.get("complete") or not status.get("finite_parameters"):
        raise ValueError("run is not complete and finite")
    if status["identities"] != identities:
        raise ValueError("completed run provenance differs from current frozen protocol")
    for stage, digest in status["checkpoint_sha256"].items():
        if sha256(run / f"{stage}.pt") != digest:
            raise ValueError("checkpoint identity changed")
    return status


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dataset", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--stage", choices=("train", "evaluate", "all"), default="all")
    parser.add_argument("--seeds", default="0,1,2,3,4,5,6,7")
    parser.add_argument("--hidden-sizes", default="128")
    parser.add_argument("--epochs", type=int, default=160)
    parser.add_argument("--batch-size", type=int, default=64)
    parser.add_argument("--learning-rate", type=float, default=1e-3)
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--device", default="cpu")
    parser.add_argument("--normalization-config", type=Path, required=True)
    parser.add_argument("--exploratory-after-failed-probe", type=Path,
                        help="Frozen diagnostic amendment; does not change or pass the failed probe gate")
    args = parser.parse_args()
    torch.set_num_threads(args.threads)
    analysis, amendment = admit_policy_run(args.dataset, args.probe, args.exploratory_after_failed_probe)
    seeds = unique_integer_list(args.seeds, "seeds")
    hidden_sizes = unique_integer_list(args.hidden_sizes, "hidden_sizes", minimum=1)
    validate_frozen_diagnostic_policy(amendment, seeds=seeds, hidden_sizes=hidden_sizes,
        epochs=args.epochs, batch_size=args.batch_size, learning_rate=args.learning_rate)
    rows = load_dataset(args.dataset)
    args.output.mkdir(parents=True, exist_ok=True)
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    from .normalize import canonical_identity
    normalization_config = json.loads(args.normalization_config.read_text())
    normalization_config.pop("identity", None)
    identities = {**analysis, "dataset_sha256": sha256(args.dataset), "policy_source_sha256": sha256(Path(__file__)),
                  "source_files_sha256": {name: sha256(Path(__file__).with_name(name)) for name in ("policy.py", "probe.py", "statistics.py", "normalize.py")},
                  "sensor_schema_identity": normalization_config["frame_schema"],
                  "raw_sensor_schema_identity": normalization_config["raw_schema"],
                  "normalization_identity": canonical_identity(normalization_config),
                  "normalization_file_sha256": sha256(args.normalization_config)}
    config = {**analysis, "schema_version": SCHEMA, "seeds": seeds, "hidden_sizes": hidden_sizes, "epochs": args.epochs,
              "batch_size": args.batch_size, "learning_rate": args.learning_rate, "device": args.device,
              "reward": "1-min(measured_latency/deadline,2); measured failure=-1",
              "selection": "validation utility only", "identities": identities, "source_commit": commit,
              "split_audit": validate_rows(rows), "PPO": False, "DAgger": False,
              "DAgger_rationale": "one physical resource decision per episode; actions do not affect the archived next-row distribution"}
    config_path = args.output / "training_config.json"
    if args.stage != "evaluate":
        if config_path.exists():
            if json.loads(config_path.read_text()) != config:
                raise ValueError("resume requires identical frozen training protocol")
        else:
            json_write(config_path, config)
    run_paths, summaries = [], []
    for hidden in hidden_sizes:
        for seed in seeds:
            for mode in ("BODY", "BLIND"):
                run = args.output / "runs" / f"gru{hidden}-s{seed}-{mode.lower()}"
                run_paths.append(run)
                if args.stage != "evaluate":
                    if (run / "status.json").exists():
                        summary = verified_completed_run(run, identities)
                    else:
                        summary = train_one([r for r in rows if r["split"] == "train"], [r for r in rows if r["split"] == "validation"], run,
                            seed=seed, hidden_size=hidden, mode=mode, epochs=args.epochs, batch_size=args.batch_size,
                            learning_rate=args.learning_rate, source_commit=commit, identities=identities, device=args.device)
                    summaries.append(summary)
                    json_write(args.output / "run_summary.json", {**analysis, "schema_version": SCHEMA, "rows": summaries, "completed_runs": len(summaries), "planned_runs": len(seeds) * len(hidden_sizes) * 2})
                    print(json.dumps({"run": run.name, "best_validation_utility": summary["best_validation_utility"]}), flush=True)
                else:
                    status = verified_completed_run(run, identities)
                    summaries.append(status)
    json_write(args.output / "run_summary.json", {**analysis, "schema_version": SCHEMA, "rows": summaries})
    if args.stage in ("evaluate", "all"):
        if (args.output / "ablation_results.json").exists():
            raise FileExistsError("refusing to overwrite frozen held-out evaluation")
        evaluate_models(rows, run_paths, args.output)


if __name__ == "__main__":
    main()
