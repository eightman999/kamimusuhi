"""Locked action-conditioned ridge outcome probe; validation only, no test path."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np

from .data import ACTION_NAMES, PRIMARY_MODES, encode_inputs, json_write, load_split, sha256, utilities, validate_disjoint

SCHEMA = 'k0-f2-probe-v1'
RIDGE_LAMBDA = 10.0
BODY_SCALE_FLOOR = .05
TARGETS = ('log_latency', 'utility')
OUTPUT_ARTIFACTS = (
    'probe_config.json', 'probe_validation.json', 'probe_models.json',
    'probe_predictions.jsonl', 'probe_selections.jsonl', 'probe_inputs.jsonl',
    'probe_shuffle_mapping.jsonl', 'study_status.json',
)


def features(rows, mode, seed=0):
    inputs, mapping = encode_inputs(rows, mode, seed, return_mapping=True)
    features = []
    for x in inputs:
        x = x.numpy().astype(np.float64)
        task = x[-1, :4]
        factor = task[0] ** 3 * task[1]
        mean = x[:, 4:].mean(0)
        features.append(np.concatenate((task, [task[0] ** 2, factor], x[-1, 4:], mean,
                                        factor * x[-1, 4:24], factor * mean[:20])))
    return np.stack(features), mapping


def target_values(rows, target):
    if target == 'log_latency':
        return np.log(np.clip(np.asarray([r['costs_seconds'] for r in rows], float), 1e-8, 20))
    if target == 'utility':
        return np.stack([utilities(r) for r in rows])
    raise ValueError('unknown locked outcome target')


def fit_ridge(x, y):
    x, y = np.asarray(x, float), np.asarray(y, float)
    if x.ndim != 2 or x.shape[1] != 126 or y.shape != (len(x), 4):
        raise ValueError('ridge requires 126 features and four candidate-action outcome heads')
    center, scale = x.mean(0), x.std(0)
    scale[:6] = np.where(scale[:6] < 1e-8, 1, scale[:6])
    scale[6:] = np.maximum(scale[6:], BODY_SCALE_FLOOR)
    ycenter, yscale = y.mean(0), y.std(0)
    yscale = np.where(yscale < 1e-8, 1, yscale)
    design = np.column_stack((np.ones(len(x)), (x - center) / scale))
    penalty = np.eye(design.shape[1]) * RIDGE_LAMBDA
    penalty[0, 0] = 0
    coefficients = np.linalg.solve(design.T @ design + penalty, design.T @ ((y - ycenter) / yscale))
    if not np.isfinite(coefficients).all():
        raise FloatingPointError('nonfinite fitted probe')
    return dict(x_center=center, x_scale=scale, y_center=ycenter, y_scale=yscale,
                coefficients=coefficients, architecture='ridge_action_heads', ridge_lambda=RIDGE_LAMBDA,
                scale_floor=BODY_SCALE_FLOOR, feature_dimension=126, action_count=4)


def predict(model, x, target):
    design = np.column_stack((np.ones(len(x)), (x - model['x_center']) / model['x_scale']))
    output = (design @ model['coefficients']) * model['y_scale'] + model['y_center']
    if not np.isfinite(output).all():
        raise FloatingPointError('nonfinite probe prediction')
    bounds = (np.log(1e-8), np.log(20)) if target == 'log_latency' else (-1, 1)
    return np.clip(output, *bounds)


def run_probe(train, validation):
    if any(r['split'] != 'train' for r in train) or any(r['split'] != 'validation' for r in validation):
        raise ValueError('probe accepts only train and validation; no test stage exists')
    validate_disjoint(train, validation)
    rows, selection_rows, predictions, selections, input_rows, mappings = [], [], [], [], [], []
    models = {}
    ytrain = {target: target_values(train, target) for target in TARGETS}
    yval = {target: target_values(validation, target) for target in TARGETS}
    nonconstant = {target: bool(np.std(ytrain[target]) > 1e-8 and np.std(yval[target]) > 1e-8) for target in TARGETS}
    for mode in PRIMARY_MODES:
        xt, train_mapping = features(train, mode)
        xv, val_mapping = features(validation, mode)
        for split, dataset, matrix, mapping in (('train', train, xt, train_mapping), ('validation', validation, xv, val_mapping)):
            if mapping:
                mappings.extend(mapping)
            input_rows.extend(dict(episode_id=r['episode_id'], session_id=r['session_id'], block_id=r['block_id'],
                split=split, mode=mode, features=feature.tolist(), feature_dimension=126)
                for r, feature in zip(dataset, matrix))
        estimated = {}
        for target in TARGETS:
            model = fit_ridge(xt, ytrain[target])
            models[mode + '/' + target] = {k: v.tolist() if isinstance(v, np.ndarray) else v for k, v in model.items()}
            estimated[target] = predict(model, xv, target)
            error = np.abs(estimated[target] - yval[target])
            denominator = float(np.std(ytrain[target]))
            rows.append(dict(mode=mode, target=target, split='validation', n_episodes=len(validation), n_actions=4,
                mae=float(error.mean()), normalized_mae=float(error.mean()/denominator) if denominator > 1e-8 else None,
                normalization_denominator_train_target_sd=denominator, per_action_mae=error.mean(0).tolist()))
            for i, r in enumerate(validation):
                predictions.extend(dict(episode_id=r['episode_id'], session_id=r['session_id'], block_id=r['block_id'],
                    split='validation', mode=mode, target=target, candidate_action=action,
                    observed=float(yval[target][i, action]), predicted=float(estimated[target][i, action])) for action in range(4))
        selected = estimated['utility'].argmax(1)
        observed_utility = yval['utility']
        regret = observed_utility.max(1) - observed_utility[np.arange(len(validation)), selected]
        accuracy = regret <= 1e-12
        selection_rows.append(dict(mode=mode, n_episodes=len(validation), selection_regret=float(regret.mean()),
                                   accuracy=float(accuracy.mean())))
        selections.extend(dict(episode_id=r['episode_id'], session_id=r['session_id'], block_id=r['block_id'],
            mode=mode, predicted_utility=estimated['utility'][i].tolist(), actual_utilities=observed_utility[i].tolist(),
            selected_action=int(selected[i]), selection_regret=float(regret[i]), accuracy=bool(accuracy[i]))
            for i, r in enumerate(validation))
    error = {(r['mode'], r['target']): r['mae'] for r in rows}
    regret = {r['mode']: r['selection_regret'] for r in selection_rows}
    checks = {'both_targets_nonconstant': all(nonconstant.values())}
    for mode in ('BLIND', 'SHUFFLED'):
        for target in TARGETS:
            checks[f'{target}_BODY_better_than_{mode}'] = error['BODY', target] < error[mode, target]
        checks[f'regret_BODY_better_than_{mode}'] = regret['BODY'] < regret[mode]
    passed = all(checks.values())
    result = dict(schema_version=SCHEMA, status='PASS' if passed else 'FAIL_AT_PROBE_GATE',
        gate=dict(**{'pass': passed}, checks=checks, nonconstant_targets=nonconstant,
                  criterion='both outcome-target MAEs and utility-head selection regret strictly improve against BLIND and SHUFFLED'),
        rows=rows, selection_rows=selection_rows, test_evaluated=False, fit_split='train',
        architecture='ridge_action_heads', candidate_actions=list(ACTION_NAMES), ridge_lambda=RIDGE_LAMBDA,
        scale_floor=BODY_SCALE_FLOOR, feature_dimension=126,
        target_transform={'log_latency': 'ln(clip(measured_seconds,1e-8,20))', 'utility': '1-min(measured_seconds/deadline,2); failure=-1'},
        selection='argmax predicted utility; ties resolved by fixed action order',
        accuracy='chosen observed utility within 1e-12 of observed maximum; diagnostic, not gate',
        inferential_unit_note='validation gate is descriptive; no telemetry-point p-value')
    return result, models, predictions, selections, input_rows, mappings


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--train-dataset', type=Path, required=True)
    parser.add_argument('--validation-dataset', type=Path, required=True)
    parser.add_argument('--protocol-lock', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    from .protocol import load_lock, lock_sha256
    lock = load_lock(args.protocol_lock, verify_sources=True)
    lock_hash = lock_sha256(args.protocol_lock)
    if any((args.output / name).exists() for name in OUTPUT_ARTIFACTS):
        raise FileExistsError('probe output is immutable; use the locked fresh output location')
    train = load_split(args.train_dataset, 'train', lock_hash, lock['normalization_identity'])
    val = load_split(args.validation_dataset, 'validation', lock_hash, lock['normalization_identity'])
    if lock['probe']['architecture'] != 'ridge_action_heads' or lock['probe']['ridge_lambda'] != 10 or lock['probe']['scale_floor'] != .05:
        raise ValueError('runtime probe differs from confirmatory lock')
    result, models, predictions, selections, inputs, mappings = run_probe(train, val)
    identities = dict(protocol_lock_sha256=lock_hash, source_commit=lock['source_commit'],
        train_dataset_sha256=sha256(args.train_dataset), validation_dataset_sha256=sha256(args.validation_dataset),
        normalization_identity=lock['normalization_identity'], source_files_sha256=lock['source_files_sha256'])
    result.update(identities)
    json_write(args.output/'probe_config.json', dict(**identities, architecture='ridge_action_heads',
        feature_dimension=126, ridge_lambda=10, scale_floor=.05, targets=list(TARGETS)))
    json_write(args.output/'probe_validation.json', result)
    json_write(args.output/'probe_models.json', dict(**identities, models=models))
    for filename, records in [('probe_predictions.jsonl', predictions), ('probe_selections.jsonl', selections),
                              ('probe_inputs.jsonl', inputs), ('probe_shuffle_mapping.jsonl', mappings)]:
        (args.output/filename).write_text(''.join(json.dumps(dict(record, protocol_lock_sha256=lock_hash), allow_nan=False)+'\n' for record in records))
    if not result['gate']['pass']:
        json_write(args.output/'study_status.json', dict(**identities, status='FAIL_AT_PROBE_GATE', test_evaluated=False, policy_trained=False))
    print(json.dumps(result['gate']), flush=True)


if __name__ == '__main__':
    main()
