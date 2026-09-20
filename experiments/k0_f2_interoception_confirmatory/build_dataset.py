"""Build immutable split files from complete, fresh, locked session collections."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np

from .data import json_write, sha256, validate_disjoint, validate_rows
from .protocol import SESSIONS, TASKS, load_lock, lock_receipt, lock_sha256

COLLECTION_SCHEMA = 'k0-f2-collection-manifest-v1'
SPLIT_SCHEMA = 'k0-f2-split-manifest-v1'


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read_json(path):
    return json.loads(Path(path).read_text())


def jsonl_bytes(path):
    contents = Path(path).read_bytes()
    require(bool(contents) and contents.endswith(b'\n'), 'complete nonempty newline-terminated JSONL required')
    lines = contents.splitlines()
    require(all(line.strip() for line in lines), 'blank JSONL record is not permitted')
    return contents, [json.loads(line) for line in lines]


def test_preflight(artifacts, lock, identity):
    """Check the probe and all frozen hashes before touching session D."""
    gate_path = artifacts / 'validation/probe_validation.json'
    gate = read_json(gate_path)
    checks = {'both_targets_nonconstant'} | {f'{target}_BODY_better_than_{mode}'
        for target in ('log_latency', 'utility', 'regret') for mode in ('BLIND', 'SHUFFLED')}
    require(gate.get('schema_version') == 'k0-f2-probe-v1' and gate.get('status') == 'PASS'
        and gate.get('gate', {}).get('pass') is True and gate.get('test_evaluated') is False,
        'session D must stay unopened after failed or missing probe')
    require(set(gate['gate']['checks']) == checks and all(v is True for v in gate['gate']['checks'].values()),
            'all locked probe checks must pass')
    freeze = read_json(artifacts / 'train/checkpoint_manifest.json')
    for document in (gate, freeze):
        require(document.get('protocol_lock_sha256') == identity
            and document.get('source_commit') == lock['source_commit']
            and document.get('source_files_sha256') == lock['source_files_sha256']
            and document.get('normalization_identity') == lock['normalization_identity'],
            'probe/checkpoint source or normalization identity mismatch')
        for split in ('train', 'validation'):
            require(document.get(split + '_dataset_sha256') == sha256(artifacts / split / 'dataset.jsonl'),
                    'probe/checkpoint split identity mismatch')
    require(freeze.get('complete') is True and freeze.get('completed_runs') == 24
        and freeze.get('checkpoint_count') == 72 and freeze.get('probe_sha256') == sha256(gate_path),
        'all 24 policies and 72 checkpoints must be frozen before D')
    expected = {(seed, mode) for seed in range(12) for mode in ('BODY', 'BLIND')}
    seen, initials = set(), {}
    for row in freeze['rows']:
        key = (row['seed'], row['training_mode'])
        require(key in expected and key not in seen and row['architecture'] == 'GRU128',
                'duplicate, missing, or unexpected frozen policy')
        seen.add(key)
        require(set(row['checkpoint_sha256']) == {'initial', 'best', 'final'}, 'all checkpoint stages required')
        run = artifacts / 'train/runs' / f'gru128-s{key[0]}-{key[1].lower()}'
        for stage, digest in row['checkpoint_sha256'].items():
            require(sha256(run / (stage + '.pt')) == digest, 'frozen checkpoint bytes changed')
        initials[key] = row['initial_parameter_sha256']
    require(seen == expected, 'exactly 12 BODY and 12 independent BLIND policies required')
    require(all(initials[seed, 'BODY'] == initials[seed, 'BLIND'] for seed in range(12)),
            'same-seed initial parameters differ')
    return freeze


def read_session(artifacts, session_id, lock, identity, receipt):
    directory = artifacts / 'collection' / session_id
    config = read_json(directory / 'collection_config.json')
    runtime = read_json(directory / 'collection_runtime_state.json')
    split = SESSIONS[session_id]['split']
    require(config.get('schema_version') == 'k0-f2-collection-v1', 'collection config schema mismatch')
    for doc in (config, runtime):
        require(doc.get('session_id') == session_id and doc.get('split') == split
            and doc.get('protocol_lock_sha256') == identity, 'session config/runtime identity mismatch')
    for key in ('source_commit', 'protocol_lock_commit', 'locked_at'):
        require(config.get(key) == receipt[key], 'session source/lock receipt mismatch')
    require(config.get('tasks') == TASKS and config.get('seed') == SESSIONS[session_id]['seed']
        and config.get('blocks') == 16 and config.get('block_seconds') == lock['collection']['block_seconds'],
        'session acquisition configuration differs from lock')
    require(all(runtime.get(k) is True for k in ('collection_complete', 'owned_sensor_stopped', 'owned_background_stopped')),
            'incomplete collection or active owned process')
    begin, end = runtime['start_timestamp'], runtime['end_timestamp']
    require(math.isfinite(begin) and math.isfinite(end) and lock['locked_at'] < begin < end,
            'session was not collected after the protocol lock')
    raw_summaries, raw_hashes = {}, {}
    for node in ('master', 'mac'):
        path = directory / f'raw_{node}_telemetry.jsonl'
        contents, raw = jsonl_bytes(path)
        require(all(r.get('schema_version') == 'k0f.raw.v1' and r.get('node_id') == node for r in raw),
                'raw source schema/node mismatch')
        times, sequences = [r['timestamp'] for r in raw], [r['sequence'] for r in raw]
        require(all(math.isfinite(t) and t > lock['locked_at'] for t in times)
            and times == sorted(times) and all(a < b for a, b in zip(sequences, sequences[1:])),
            'raw telemetry time/sequence is not fresh and monotonic')
        raw_summaries[node] = dict(source_start_timestamp=times[0], source_end_timestamp=times[-1],
            samples=len(raw), first_sequence=sequences[0], last_sequence=sequences[-1])
        raw_hashes['raw_' + node + '_sha256'] = hashlib.sha256(contents).hexdigest()
    frame_bytes, frames = jsonl_bytes(directory / 'interoceptive_frames.jsonl')
    frame_index = {}
    for frame in frames:
        require(frame.get('schema_version') == 'k0f2.frame.v1' and frame.get('source_kind') == 'real'
            and frame.get('session_id') == session_id and frame.get('protocol_lock_sha256') == identity
            and frame.get('normalization_identity') == lock['normalization_identity'], 'frame identity mismatch')
        require(frame['frame_id'] not in frame_index, 'duplicate telemetry frame id')
        require(math.isfinite(frame['timestamp']) and begin <= frame['timestamp'] <= end,
                'frame outside collection interval')
        v, m = np.asarray(frame['values'], float), np.asarray(frame['mask'], float)
        require(v.shape == m.shape == (20,) and np.isfinite(v).all() and np.all((v >= 0) & (v <= 1))
            and np.isin(m, [0, 1]).all() and np.all(v[m == 0] == 0), 'invalid normalized frame')
        frame_index[frame['frame_id']] = frame
    contents, rows = jsonl_bytes(directory / 'dataset.jsonl')
    validate_rows(rows, split, identity, lock['normalization_identity'])
    require(all(r['session_id'] == session_id and not r.get('fixture_only') for r in rows),
            'wrong session or synthetic fixture in collection')
    blocks = {}
    for row in rows:
        require(begin <= row['timestamp'] <= end, 'decision outside collection interval')
        blocks.setdefault(row['block_id'], []).append((row['task_id'], len(row['body_sequence'])))
        for prefix in ('', 'stale_'):
            p = row['provenance']
            for index, fid in enumerate(p[prefix + 'frame_ids']):
                require(fid in frame_index, 'row references missing source frame')
                frame = frame_index[fid]
                expected = list(frame['values'])
                if prefix:
                    expected[18] = min(1., expected[18] + max(0., row['timestamp'] - frame['timestamp']) / 60.)
                require(p[prefix + 'frame_timestamps'][index] == frame['timestamp']
                    and row[prefix + 'body_mask_sequence'][index] == frame['mask']
                    and np.allclose(row[prefix + 'body_sequence'][index], expected, atol=1e-7, rtol=0),
                    'body row is not reconstructed from its archived source frame')
    cases = {(t, length) for t in range(4) for length in range(1, 9)}
    require(len(rows) == 512 and len(blocks) == 16
        and all(len(value) == 32 and set(value) == cases for value in blocks.values()),
        'complete session requires 16 blocks and 512 fixed-design decisions')
    entry = dict(session_id=session_id, split=split, relative_path=f'collection/{session_id}',
        **receipt, start_timestamp=begin, end_timestamp=end, runtime_complete=True,
        blocks=len(blocks), decisions=len(rows), rows=len(rows), raw_sources=raw_summaries,
        raw_source_start_timestamp=min(r['source_start_timestamp'] for r in raw_summaries.values()),
        raw_source_end_timestamp=max(r['source_end_timestamp'] for r in raw_summaries.values()),
        **raw_hashes, frames_sha256=hashlib.sha256(frame_bytes).hexdigest(),
        dataset_sha256=hashlib.sha256(contents).hexdigest(),
        collection_config_sha256=sha256(directory / 'collection_config.json'),
        runtime_state_sha256=sha256(directory / 'collection_runtime_state.json'),
        source_files_sha256=lock['source_files_sha256'], normalization_identity=lock['normalization_identity'])
    return entry, contents, rows, set(frame_index)


def build(artifacts, protocol_lock, include_test=False):
    artifacts, protocol_lock = Path(artifacts), Path(protocol_lock)
    lock = load_lock(protocol_lock, verify_sources=True)
    identity, receipt = lock_sha256(protocol_lock), lock_receipt(protocol_lock)
    if include_test:
        freeze = test_preflight(artifacts, lock, identity)
        require(freeze.get('lock_commit') == receipt['protocol_lock_commit'], 'checkpoint lock commit mismatch')
    session_ids = ('A', 'B', 'C', 'D') if include_test else ('A', 'B', 'C')
    entries, data, all_frames = [], {}, set()
    for sid in session_ids:
        entry, contents, rows, frames = read_session(artifacts, sid, lock, identity, receipt)
        require(not frames & all_frames, 'source frames overlap independent sessions')
        all_frames |= frames
        entries.append(entry)
        data[sid] = (contents, rows)
    raw_hashes = [e[key] for e in entries for key in ('raw_master_sha256', 'raw_mac_sha256')]
    require(len(set(raw_hashes)) == len(raw_hashes), 'raw telemetry content reused across sessions')
    for first, second in zip(entries, entries[1:]):
        require(second['start_timestamp'] - first['end_timestamp'] >= lock['collection']['inter_session_buffer_seconds'],
                'session overlap or insufficient locked temporal buffer')
    plans, splits, split_rows = {}, {}, []
    for split, sessions in (('train', ('A', 'B')), ('validation', ('C',)), ('test', ('D',))):
        if split == 'test' and not include_test:
            continue
        contents = b''.join(data[s][0] for s in sessions)
        rows = [r for s in sessions for r in data[s][1]]
        validate_rows(rows, split, identity, lock['normalization_identity'], require_complete=True)
        split_rows.append(rows)
        relative = split + '/dataset.jsonl'
        plans[relative] = contents
        splits[split] = dict(dataset=relative, sha256=hashlib.sha256(contents).hexdigest(),
            sessions=list(sessions), rows=len(rows), blocks=len({r['block_id'] for r in rows}))
    validate_disjoint(*split_rows)
    context = dict(**receipt, source_files_sha256=lock['source_files_sha256'],
        normalization_identity=lock['normalization_identity'])
    collection = dict(schema_version=COLLECTION_SCHEMA, **context, sessions=entries,
        complete=include_test, planned_sessions=['A', 'B', 'C', 'D'], available_sessions=list(session_ids))
    manifest = dict(schema_version=SPLIT_SCHEMA, **context, splits=splits, test_included=include_test,
        concatenation='exact source JSONL bytes, A then B for train; no reserialization')
    # Check every destination before writing any file. Existing A/B/C records may only gain D.
    for relative, contents in plans.items():
        path = artifacts / relative
        if path.exists():
            require(path.read_bytes() == contents, 'refusing to change immutable split bytes: ' + relative)
    for filename, planned, key in (('collection_manifest.json', collection, 'sessions'),
                                    ('split_manifest.json', manifest, 'splits')):
        path = artifacts / filename
        if path.exists():
            previous = read_json(path)
            prior_items = {v['session_id']: v for v in previous[key]} if key == 'sessions' else previous[key]
            next_items = {v['session_id']: v for v in planned[key]} if key == 'sessions' else planned[key]
            require(all(k in next_items and next_items[k] == v for k, v in prior_items.items()),
                    'existing manifest entries changed or would be removed')
            require(all(previous.get(k) == v for k, v in context.items())
                    and previous.get('schema_version') == planned['schema_version'], 'manifest identity changed')
    for relative, contents in plans.items():
        path = artifacts / relative
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open('xb') as handle:
                handle.write(contents)
    json_write(artifacts / 'collection_manifest.json', collection)
    json_write(artifacts / 'split_manifest.json', manifest)
    return collection, manifest


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--protocol-lock', type=Path, required=True)
    parser.add_argument('--include-test', action='store_true')
    args = parser.parse_args()
    _, result = build(args.artifacts, args.protocol_lock, args.include_test)
    print(json.dumps(dict(splits=result['splits'], test_included=result['test_included'])), flush=True)


if __name__ == '__main__':
    main()
