"""F2 locked paired supervised policies; failed probe never admits test data."""
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
from torch.nn.utils.rnn import pack_padded_sequence,pad_sequence
from .data import (SCHEMA,BODY_DIM,TASK_DIM,ACTION_NAMES,PRIMARY_MODES,TEMPORAL_OOD,SENSOR_OOD,RESOURCE_OOD,
                   encode_inputs,utilities,load_split,validate_disjoint,json_write,sha256,shuffle_mapping)
from .statistics import describe,paired_comparison

def analysis_metadata(identities=None):
    identities = identities or {}
    return dict(experiment_scope='confirmatory_f2', confirmatory_eligible=True,
                protocol_lock_sha256=identities.get('protocol_lock_sha256'),
                source_commit=identities.get('source_commit'))

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
              mode="BODY", epochs=240, batch_size=128, learning_rate=1e-3,
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
    for stage, state in (("initial", initial_state), ("best", best_state), ("final", final_state)):
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
    metadata["checkpoint_sha256"] = {stage: sha256(output / f"{stage}.pt") for stage in ("initial", "best", "final")}
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
                eligible = [bi for bi in indices if rows[ai]["block_id"] != rows[bi]["block_id"]]
                if not eligible:
                    continue
                fork_hidden = None if hidden is None else hidden.repeat(1, len(eligible), 1).clone()
                fork_logits, fork_states = model([inputs[bi][-1:].to(device) for bi in eligible], fork_hidden)
                require_finite_tensors([fork_logits, fork_states], "counterfactual states")
                fork_actions = fork_logits.argmax(-1).cpu().tolist()
                for bi, action_b in zip(eligible, fork_actions):
                    matched.append(action_b)
                    frozen.append(action_a)
                    destinations.append(rows[bi])
                    details.append({"source_episode_id": rows[ai]["episode_id"],
                        "destination_episode_id": rows[bi]["episode_id"],
                        "frozen_action": ACTION_NAMES[action_a], "matched_action": ACTION_NAMES[action_b],
                        "frozen_utility": float(utilities(rows[bi])[action_a]),
                        "matched_utility": float(utilities(rows[bi])[action_b])})
    if not destinations:
        return {"seed": seed, "architecture": f"GRU{model.hidden_size}", "n_pairs": 0, "available": False, "reason": "no repeated exact tasks across blocks"}, []
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
    shuffle_records = []
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
        if training_mode == "BODY":
            shuffle_records.extend(shuffle_mapping(test, seed))
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
                    "action": ACTION_NAMES[action], "hidden_norm": norm, "task_features": row["task_features"], "checkpoint": "best",
                    "source_kind": "real_telemetry_measured_cost_replay",
                    "timestamp": row.get("timestamp", row.get("provenance", {}).get("timestamp")),
                    "body_values": row["body_sequence"][-1], "body_mask": row["body_mask_sequence"][-1],
                    "utility": float(utilities(row)[action]), "latency_seconds": row["costs_seconds"][action]})
        if training_mode == "BODY":
            result, detail = counterfactual(model, test, seed)
            counter.append(result)
            forks.extend(dict(d, seed=seed, architecture=meta["architecture"]) for d in detail)
            for mode in TEMPORAL_OOD + SENSOR_OOD + RESOURCE_OOD:
                actions, _ = infer(model, encode_inputs(test, mode, seed))
                outcomes = test
                if mode in RESOURCE_OOD:
                    outcomes = copy.deepcopy(test)
                    for row in outcomes:
                        if mode == 'controlled_rtx_unavailable':
                            row['failures'][1] = True
                            row['failures'][3] = True
                        elif mode == 'controlled_p100_unavailable':
                            row['failures'][2] = True
                        else:
                            row['costs_seconds'][0] *= 2
                ood.append(dict(seed=seed, architecture=meta["architecture"], mode=mode,
                    category="temporal" if mode in TEMPORAL_OOD else "sensor",
                    kind="synthetic_resource_failure_simulation" if mode in RESOURCE_OOD else "synthetic_observation_perturbation",
                    outcome_provenance="locked_resource_constraint_simulation; unaffected_actions_measured_cost" if mode in RESOURCE_OOD else "measured_action_cost_replay",
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
            comparisons[{"BLIND": "D_MASKED_BLIND", "SHUFFLED": "P2", "STALE": "P3"}[mode]] = paired_comparison([r["utility"] for r in table["BODY"]], [r["utility"] for r in table[mode]])
        independent = sorted([r for r in ablations if r["architecture"] == architecture and r["training_mode"] == "BLIND"], key=lambda r: r["seed"])
        if independent and [r["seed"] for r in independent] == [r["seed"] for r in table["BODY"]]:
            comparisons["P1"] = paired_comparison([r["utility"] for r in table["BODY"]], [r["utility"] for r in independent])
    json_write(output / "test_results.json", {**analysis, "schema_version": SCHEMA, "rows": ablations, "comparisons": comparisons, "statistics": aggregates,
        "primary_control": "P1 independent BLIND, P2 matched SHUFFLED, P3 STALE30; masked BODY checkpoint is diagnostic", "outcome_provenance": "measured_action_cost_replay"})
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
    json_write(output / "counterfactual_results.json", {**analysis, "schema_version": SCHEMA, "rows": counter, "comparisons": {"CF": counter_stats["GRU128"]} if "GRU128" in counter_stats else {}, "statistics": counter_descriptive,
        "limitation": "Archived real outcomes; body swapping is controlled in model replay, not a randomized physical intervention."})
    ood_statistics = {f"{arch}_{mode}": {metric: describe([r[metric] for r in ood if r["architecture"] == arch and r["mode"] == mode])
        for metric in ("utility", "latency_seconds", "deadline_success_rate", "failure_rate", "oracle_match_rate")}
        for arch in sorted(set(r["architecture"] for r in ood)) for mode in TEMPORAL_OOD + SENSOR_OOD + RESOURCE_OOD}
    json_write(output / "ood_results.json", {**analysis, "schema_version": SCHEMA, "rows": ood, "statistics": ood_statistics,
        "limitation": "Synthetic observation changes scored against fixed measured costs; no real network fault, GPU failure, or unsafe workload induced.",
        "rtx3060_sensor_unavailable_scope": "RTX3060 sensor channels missing; measured costs unchanged",
        "resource_simulation_scope": "RTX absence fails RTX and WAIT; P100 absence fails P100; CPU constraint doubles CPU latency; hardware is unchanged"})
    for filename, entries in (("policy_traces.jsonl", traces), ("counterfactual_pairs.jsonl", forks), ("policy_input_traces.jsonl", input_traces), ("test_shuffle_mapping.jsonl", shuffle_records)):
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


def gate_preflight(probe_path, protocol_lock_sha256, output):
    """Reject failed gate before any training or test dataset loader is called."""
    probe = json.loads(Path(probe_path).read_text())
    if probe.get('schema_version') != 'k0-f2-probe-v1' or probe.get('protocol_lock_sha256') != protocol_lock_sha256:
        raise ValueError('probe is not from this F2 confirmatory lock')
    if probe.get('test_evaluated') is not False or type(probe.get('gate', {}).get('pass')) is not bool:
        raise ValueError('invalid probe gate record')
    if not probe['gate']['pass']:
        context = dict(protocol_lock_sha256=protocol_lock_sha256, source_commit=probe['source_commit'],
                       status='FAIL_AT_PROBE_GATE', probe_sha256=sha256(probe_path), policy_trained=False,
                       test_dataset_opened=False, test_evaluated=False)
        protected = ('training_config.json', 'checkpoint_manifest.json', 'test_results.json',
                     'counterfactual_results.json', 'ood_results.json', 'final_checkpoint_results.json')
        if any((Path(output)/name).exists() for name in protected):
            raise FileExistsError('failed gate cannot overwrite prior policy or evaluation artifacts')
        json_write(Path(output)/'study_status.json', context)
        for name in ('test_results.json','counterfactual_results.json','ood_results.json','final_checkpoint_results.json'):
            json_write(Path(output)/name, dict(context, status='NOT_RUN_PROBE_GATE_FAILED', rows=[]))
        return None
    if probe.get('status') != 'PASS' or not all(probe['gate']['checks'].values()):
        raise ValueError('inconsistent passing gate')
    expected_checks = {'both_targets_nonconstant'} | {f'{target}_BODY_better_than_{mode}'
        for target in ('log_latency', 'utility', 'regret') for mode in ('BLIND', 'SHUFFLED')}
    if set(probe['gate']['checks']) != expected_checks or any(v is not True for v in probe['gate']['checks'].values()):
        raise ValueError('passing gate requires every locked outcome and regret check')
    return probe


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--stage', choices=('train','evaluate'), required=True)
    parser.add_argument('--train-dataset', type=Path, required=True)
    parser.add_argument('--validation-dataset', type=Path, required=True)
    parser.add_argument('--test-dataset', type=Path)
    parser.add_argument('--probe', type=Path, required=True)
    parser.add_argument('--protocol-lock', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--threads', type=int, default=4)
    parser.add_argument('--device', default='cpu')
    args = parser.parse_args()
    from .protocol import load_lock, lock_sha256
    lock = load_lock(args.protocol_lock, verify_sources=True)
    lock_hash = lock_sha256(args.protocol_lock)
    probe = gate_preflight(args.probe, lock_hash, args.output)
    if probe is None:
        print('FAIL_AT_PROBE_GATE: no policy training or test file opened', flush=True)
        return
    policy = lock['policy']
    if (policy['architecture'], policy['seeds'], policy['epochs'], policy['batch_size'], policy['learning_rate']) != ('GRU128', list(range(12)), 240, 128, .001):
        raise ValueError('policy architecture/seeds/budget differ from F2 lock')
    if args.device != policy['device'] or args.threads != policy['torch_threads']:
        raise ValueError('policy execution device/thread count differs from F2 lock')
    stats = lock['statistics']
    if (stats['n_seeds'], stats['minimum_effect'], stats['bootstrap_resamples'], stats['bootstrap_seed'], stats['sign_alpha']) != (12, .01, 20000, 1847, .05):
        raise ValueError('statistics differ from locked F2 implementation')
    for key, path in [('train_dataset_sha256', args.train_dataset), ('validation_dataset_sha256', args.validation_dataset)]:
        if probe[key] != sha256(path):
            raise ValueError('training/validation data changed after probe gate')
    if args.stage == 'train' and args.test_dataset is not None:
        raise ValueError('training command must not receive a test dataset')
    torch.set_num_threads(args.threads)
    lock_commit = subprocess.check_output(['git','rev-parse','F2_PROTOCOL_LOCK'],text=True).strip()
    identities = dict(protocol_lock_sha256=lock_hash, source_commit=lock['source_commit'], lock_commit=lock_commit,
        source_files_sha256=lock['source_files_sha256'], normalization_identity=lock['normalization_identity'],
        sensor_schema_identity='k0f2.frame.v1', raw_sensor_schema_identity='k0f.raw.v1',
        train_dataset_sha256=sha256(args.train_dataset), validation_dataset_sha256=sha256(args.validation_dataset),
        probe_sha256=sha256(args.probe))
    config = dict(schema_version=SCHEMA, source_commit=lock['source_commit'], protocol_lock_sha256=lock_hash,
        lock_commit=lock_commit, identities=identities, architecture='GRU128', seeds=list(range(12)),
        epochs=240, batch_size=128, learning_rate=.001, device=args.device, threads=args.threads,
        training_modes=['BODY','BLIND'], loss='teacher cross_entropy + expected utility regret',
        checkpoint_selection='validation utility maximum, earliest strict improvement', PPO=False, DAgger=False,
        DAgger_reason='one resource decision per episode; complete measured action feedback and no action-dependent next-state replay')
    args.output.mkdir(parents=True,exist_ok=True)
    config_path = args.output/'training_config.json'
    if config_path.exists():
        if json.loads(config_path.read_text()) != config:
            raise ValueError('frozen training configuration changed')
    elif args.stage == 'evaluate':
        raise ValueError('test evaluation requires completed frozen training')
    else:
        json_write(config_path,config)
    run_paths = [args.output/'runs'/f'gru128-s{seed}-{mode.lower()}' for seed in range(12) for mode in ('BODY','BLIND')]
    summaries = []
    if args.stage == 'train':
        train = load_split(args.train_dataset,'train',lock_hash,lock['normalization_identity'])
        validation = load_split(args.validation_dataset,'validation',lock_hash,lock['normalization_identity'])
        split_audit = validate_disjoint(train,validation)
        json_write(args.output/'training_split_audit.json',dict(split_audit,**identities))
        for seed in range(12):
            for mode in ('BODY','BLIND'):
                run = args.output/'runs'/f'gru128-s{seed}-{mode.lower()}'
                if (run/'status.json').exists():
                    result = verified_completed_run(run,identities)
                else:
                    result = train_one(train,validation,run,seed=seed,hidden_size=128,mode=mode,
                        epochs=240,batch_size=128,learning_rate=.001,source_commit=lock['source_commit'],identities=identities,device=args.device)
                summaries.append(result)
                json_write(args.output/'run_summary.json',dict(protocol_lock_sha256=lock_hash, source_commit=lock['source_commit'],
                    rows=summaries,completed_runs=len(summaries),planned_runs=24))
                print(json.dumps(dict(run=run.name,best_validation_utility=result['best_validation_utility'])),flush=True)
    else:
        # Every checkpoint is verified before opening the separate, newly acquired D file.
        summaries = [verified_completed_run(run,identities) for run in run_paths]
        if args.test_dataset is None:
            raise ValueError('evaluation requires the separate session D test file')
        if (args.output/'test_results.json').exists():
            raise FileExistsError('one-shot test evaluation already exists; never overwrite')
        test = load_split(args.test_dataset,'test',lock_hash,lock['normalization_identity'])
        json_write(args.output/'test_dataset_identity.json',dict(protocol_lock_sha256=lock_hash,
            source_commit=lock['source_commit'],test_dataset_sha256=sha256(args.test_dataset),n_rows=len(test),
            eligible_checkpoint_count=len(summaries)*3,probe_gate_passed_before_open=True))
        evaluate_models(test,run_paths,args.output)
    manifest = [dict(seed=s['seed'],training_mode=s['training_mode'],architecture=s['architecture'],
        checkpoint_sha256=s['checkpoint_sha256'],initial_parameter_sha256=s['initial_parameter_sha256']) for s in summaries]
    json_write(args.output/'checkpoint_manifest.json',dict(**identities,rows=manifest,
        complete=len(manifest)==24, completed_runs=len(manifest), checkpoint_count=3*len(manifest)))


if __name__ == '__main__':
    main()
