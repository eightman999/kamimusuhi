"""Synthetic fixtures only; no K0-F or collected research data are loaded."""
import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import numpy as np
import torch
from torch import nn

from experiments.k0_f2_interoception_confirmatory import policy
from experiments.k0_f2_interoception_confirmatory.data import (
    ACTION_NAMES, PRIMARY_MODES, RESOURCE_OOD, SENSOR_OOD, TEMPORAL_OOD,
    encode_inputs, load_split, shuffle_mapping, task_features, utilities,
    validate_disjoint, validate_rows, TASKS,
)
from experiments.k0_f2_interoception_confirmatory.policy import BodyPolicy, counterfactual, train_one, load_model

LOCK = 'f' * 64


def fixture_rows(split='train', session='A', blocks=8, tasks=range(4), lengths=range(1, 3)):
    rows = []
    for block in range(blocks):
        busy = [0., 1., .2, .8, .4, .6, .1, .9][block % 8]
        for task_id in tasks:
            deadline = TASKS[task_id][2]
            for length in lengths:
                timestamp = 1000000 + ord(session) * 10000 + block * 100 + task_id * 10 + length
                body = np.zeros((length, 20), np.float32)
                body[:, 5], body[:, 9] = busy, 1 - busy
                body[:, 6], body[:, 10] = busy * .5, (1 - busy) * .5
                body[:, 19] = 1
                stale = body.copy()
                stale[:, 5], stale[:, 9] = 1 - busy, busy
                stale[:, 18] = .5
                eid = f'{session}-{block}-{task_id}-{length}'
                rows.append(dict(schema_version='k0-f2-policy-v1', protocol_lock_sha256=LOCK,
                    session_id=session, split=split, block_id=f'{session}-{block}', episode_id=eid,
                    task_id=task_id, timestamp=timestamp, task_features=task_features(task_id), deadline_seconds=deadline,
                    body_sequence=body.tolist(), body_mask_sequence=np.ones_like(body).tolist(),
                    stale_body_sequence=stale.tolist(), stale_body_mask_sequence=np.ones_like(stale).tolist(),
                    costs_seconds=[deadline * 1.8, deadline * (.3 + busy), deadline * (1.3 - busy), .02 + deadline * (.3 + busy)],
                    failures=[False] * 4, fixture_only=True,
                    provenance=dict(telemetry_kind='real', fixture_origin='synthetic unit-test fixture, never research',
                        normalization_identity='fixture-normalization',
                        frame_ids=[f'{eid}-current-{i}' for i in range(length)],
                        stale_frame_ids=[f'{eid}-stale-{i}' for i in range(length)],
                        frame_timestamps=[timestamp - length + i for i in range(length)],
                        stale_frame_timestamps=[timestamp - 30 - length + i for i in range(length)])))
    return rows


class BodyFixture(nn.Module):
    def __init__(self):
        super().__init__()
        self.dummy = nn.Parameter(torch.zeros(1))
        self.hidden_size = 128

    def forward(self, sequences, hidden=None):
        logits = torch.stack([torch.stack([self.dummy[0]-10, -x[-1,9], -x[-1,13], self.dummy[0]-10]) for x in sequences])
        return logits, torch.ones(1,len(sequences),1)


class PolicyTests(unittest.TestCase):
    def test_fixed_four_action_teacher_and_wait(self):
        row = fixture_rows()[0]
        self.assertEqual(len(ACTION_NAMES), 4)
        self.assertEqual(ACTION_NAMES[3], 'WAIT')
        self.assertGreaterEqual(row['costs_seconds'][3]-row['costs_seconds'][1], .02-1e-12)
        self.assertEqual(len(utilities(row)), 4)
        row['failures'][3] = True
        self.assertEqual(utilities(row)[3], -1.)

    def test_input_whitelist_and_missing_mask(self):
        row = fixture_rows()[0]
        other = copy.deepcopy(row)
        other.update(workload_label='forbidden', teacher_action=3, timestamp=9e9, costs_seconds=[99]*4)
        other['provenance']['future_utilization'] = [1,2,3]
        self.assertTrue(torch.equal(encode_inputs([row])[0], encode_inputs([other])[0]))
        missing = copy.deepcopy(row)
        missing['body_mask_sequence'][-1][5] = 0
        self.assertNotEqual(encode_inputs([missing])[0][-1,29], encode_inputs([row])[0][-1,29])
        self.assertEqual(float(encode_inputs([row],'BLIND')[0][:,4:].abs().sum()),0.)

    def test_split_schema_lock_future_and_overlap(self):
        train = fixture_rows()
        val = fixture_rows('validation','C')
        validate_rows(train,'train',LOCK)
        validate_disjoint(train,val)
        for mutation in ('schema','session','lock','future','stale'):
            bad = copy.deepcopy(train)
            if mutation=='schema':bad[0]['schema_version']='k0-f-policy-v1'
            if mutation=='session':bad[0]['session_id']='D'
            if mutation=='lock':bad[0]['protocol_lock_sha256']='x'
            if mutation=='future':bad[0]['provenance']['frame_timestamps'][-1]=bad[0]['timestamp']+1
            if mutation=='stale':bad[0]['provenance']['stale_frame_timestamps'][-1]=bad[0]['timestamp']-5
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                validate_rows(bad,'train',LOCK)
        with self.assertRaises(ValueError):validate_disjoint(train,train)
        bad=copy.deepcopy(train)
        bad[0]['body_mask_sequence'][-1][1]=.5
        with self.assertRaises(ValueError):validate_rows(bad,'train',LOCK)
        bad=copy.deepcopy(train)
        bad[0]['body_mask_sequence'][-1][9]=0
        with self.assertRaises(ValueError):validate_rows(bad,'train',LOCK)

    def test_shuffle_preserves_task_session_length_and_marginals(self):
        rows = fixture_rows()+fixture_rows('train','B')
        inputs, mapping = encode_inputs(rows,'SHUFFLED',3,return_mapping=True)
        original = encode_inputs(rows)
        self.assertEqual(len(set(r['donor_index'] for r in mapping)),len(rows))
        for row,entry,x in zip(rows,mapping,inputs):
            donor=rows[entry['donor_index']]
            self.assertEqual((row['session_id'],row['split'],row['task_id'],len(row['body_sequence'])),
                             (donor['session_id'],donor['split'],donor['task_id'],len(donor['body_sequence'])))
            self.assertNotEqual(row['block_id'],donor['block_id'])
            self.assertTrue(torch.equal(x[:,:4],original[entry['recipient_index']][:,:4]))
        self.assertEqual(sorted(x[:,4:].numpy().tobytes() for x in inputs),sorted(x[:,4:].numpy().tobytes() for x in original))
        with self.assertRaises(ValueError):shuffle_mapping(rows[:1])

    def test_all_locked_ood_finite(self):
        rows=fixture_rows()[:4]
        model=BodyPolicy(8)
        self.assertEqual(len(TEMPORAL_OOD),6)
        self.assertEqual(len(SENSOR_OOD),10)
        for mode in TEMPORAL_OOD+SENSOR_OOD+RESOURCE_OOD:
            with self.subTest(mode=mode):
                inputs=encode_inputs(rows,mode)
                actions,norms=policy.infer(model,inputs)
                self.assertTrue(np.isfinite(norms).all())
                self.assertTrue(all(0<=a<4 for a in actions))

    def test_counterfactual_complete_pairs_and_downstream_benefit(self):
        rows=fixture_rows('test','D',blocks=4,tasks=range(1),lengths=range(1,3))
        summary, pairs=counterfactual(BodyFixture(),rows,0)
        self.assertEqual(len(pairs),8*6)
        self.assertGreater(summary['action_change_rate'],0)
        self.assertGreater(summary['utility_gain'],0)

    def test_checkpoint_initial_best_final_and_paired_initialization(self):
        torch.set_num_threads(1)
        train=fixture_rows(blocks=2)
        val=fixture_rows('validation','C',blocks=2)
        with tempfile.TemporaryDirectory() as temporary:
            p=Path(temporary)
            results=[]
            for mode in ('BODY','BLIND'):
                results.append(train_one(train,val,p/mode,seed=2,hidden_size=8,mode=mode,epochs=2,batch_size=8,identities={'protocol_lock_sha256':LOCK}))
                for stage in ('initial','best','final'):
                    _,meta=load_model(p/mode/(stage+'.pt'))
                    self.assertEqual(meta['checkpoint_stage'],stage)
                    self.assertEqual(meta['protocol_lock_sha256'],LOCK)
            self.assertEqual(results[0]['initial_parameter_sha256'],results[1]['initial_parameter_sha256'])
            with self.assertRaises(ValueError):train_one(fixture_rows('test','D'),val,p/'leak',epochs=1)

    def test_gate_failure_never_opens_test_or_training_dataset(self):
        with tempfile.TemporaryDirectory() as temporary:
            p=Path(temporary);probe=p/'probe.json'
            probe.write_text(json.dumps(dict(schema_version='k0-f2-probe-v1',protocol_lock_sha256=LOCK,
                source_commit='fixture',test_evaluated=False,status='FAIL_AT_PROBE_GATE',gate={'pass':False,'checks':{}})))
            with mock.patch.object(policy,'load_split') as load:
                self.assertIsNone(policy.gate_preflight(probe,LOCK,p/'out'))
                load.assert_not_called()
            self.assertEqual(json.loads((p/'out/study_status.json').read_text())['status'],'FAIL_AT_PROBE_GATE')
            self.assertFalse((p/'never_created_test.jsonl').exists())
            with self.assertRaises(FileExistsError):policy.gate_preflight(probe,LOCK,p/'out')

    def test_failed_gate_cli_never_reads_absent_test_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            p=Path(temporary);probe=p/'probe.json'
            probe.write_text(json.dumps(dict(schema_version='k0-f2-probe-v1',protocol_lock_sha256=LOCK,
                source_commit='fixture',test_evaluated=False,status='FAIL_AT_PROBE_GATE',gate={'pass':False,'checks':{}})))
            args=['policy','--stage','evaluate','--train-dataset',str(p/'absent-train'),
                  '--validation-dataset',str(p/'absent-validation'),'--test-dataset',str(p/'absent-test'),
                  '--probe',str(probe),'--protocol-lock',str(p/'fixture-lock'),'--output',str(p/'out')]
            with mock.patch('sys.argv',args), mock.patch('experiments.k0_f2_interoception_confirmatory.protocol.load_lock',return_value={}), mock.patch('experiments.k0_f2_interoception_confirmatory.protocol.lock_sha256',return_value=LOCK), mock.patch.object(policy,'load_split') as load:
                policy.main()
                load.assert_not_called()

    def test_passing_gate_cannot_omit_regret_conditions(self):
        with tempfile.TemporaryDirectory() as temporary:
            p=Path(temporary);probe=p/'probe.json'
            probe.write_text(json.dumps(dict(schema_version='k0-f2-probe-v1',protocol_lock_sha256=LOCK,
                source_commit='fixture',test_evaluated=False,status='PASS',gate={'pass':True,'checks':{}})))
            with self.assertRaises(ValueError):policy.gate_preflight(probe,LOCK,p/'out')


if __name__=='__main__':unittest.main()
