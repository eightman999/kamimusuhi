"""Synthetic action-conditioned probe boundary tests."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import numpy as np
from experiments.k0_f2_interoception_confirmatory import probe
from experiments.k0_f2_interoception_confirmatory.probe import features,fit_ridge,predict,run_probe,target_values
from experiments.k0_f2_interoception_confirmatory.tests.test_policy import fixture_rows


class ProbeTests(unittest.TestCase):
    def test_basis_126_and_outcome_not_input(self):
        rows=fixture_rows();x,_=features(rows,'BODY')
        self.assertEqual(x.shape,(len(rows),126))
        changed=copy.deepcopy(rows)
        for row in changed:row.update(costs_seconds=[900]*4,failures=[True]*4,workload_label='secret',teacher_action=3)
        np.testing.assert_array_equal(features(changed,'BODY')[0],x)
        cf=x[:,0]**3*x[:,1]
        np.testing.assert_allclose(x[:,5],cf)
        np.testing.assert_allclose(x[:,86:106],x[:,6:26]*cf[:,None])
        np.testing.assert_allclose(x[:,106:126],x[:,46:66]*cf[:,None])

    def test_train_only_floor_and_action_head_shape(self):
        rows=fixture_rows();x,_=features(rows,'BODY');y=target_values(rows,'utility')
        x[:,7]=np.arange(len(x))*1e-7
        model=fit_ridge(x,y)
        self.assertEqual(model['coefficients'].shape,(127,4))
        self.assertEqual(model['x_scale'][7],.05)
        old=model['x_center'].copy();predict(model,x*100,'utility')
        np.testing.assert_array_equal(model['x_center'],old)
        self.assertTrue(np.all(np.abs(predict(model,x*100,'utility'))<=1))

    def test_target_transform_exact(self):
        row=fixture_rows()[0]
        np.testing.assert_allclose(target_values([row],'log_latency')[0],np.log(np.clip(row['costs_seconds'],1e-8,20)))
        row['failures'][2]=True
        self.assertEqual(target_values([row],'utility')[0,2],-1.)

    def test_validation_metrics_gate_and_no_test_stage(self):
        train=fixture_rows();val=fixture_rows('validation','C')
        result,models,predictions,selections,inputs,mapping=run_probe(train,val)
        self.assertEqual(len(result['rows']),8)
        self.assertEqual(len(result['selection_rows']),4)
        self.assertEqual(len(predictions),len(val)*4*2*4)
        self.assertFalse(result['test_evaluated'])
        self.assertEqual(result['gate']['pass'],all(result['gate']['checks'].values()))
        self.assertTrue(all(r['split']=='validation' for r in predictions))
        self.assertTrue(all(len(r['predicted_utility'])==4 for r in selections))
        self.assertTrue(all(m['feature_dimension']==126 for m in models.values()))
        with self.assertRaises(ValueError):run_probe(train,fixture_rows('test','D'))

    def test_constant_second_target_blocks_gate(self):
        train=fixture_rows();val=fixture_rows('validation','C')
        for row in train+val:row['failures']=[True]*4
        result,*_=run_probe(train,val)
        self.assertFalse(result['gate']['pass'])
        self.assertFalse(result['gate']['nonconstant_targets']['utility'])
        self.assertEqual(result['status'],'FAIL_AT_PROBE_GATE')

    def test_cli_dataset_coexists_and_owned_artifacts_are_immutable(self):
        lock = dict(normalization_identity='fixture', source_commit='fixture', source_files_sha256={},
                    probe=dict(architecture='ridge_action_heads', ridge_lambda=10, scale_floor=.05))
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            train_path = base / 'train' / 'dataset.jsonl'
            output = base / 'validation'
            val_path = output / 'dataset.jsonl'
            train_path.parent.mkdir()
            output.mkdir()
            train_path.write_bytes(b'{"fixture":"train"}\n')
            val_path.write_bytes(b'{"fixture":"validation"}\n')
            before = val_path.read_bytes()
            argv = ['probe', '--train-dataset', str(train_path), '--validation-dataset', str(val_path),
                    '--protocol-lock', str(base / 'lock.json'), '--output', str(output)]
            fixture_result = dict(gate={'pass': False}, status='FAIL_AT_PROBE_GATE', test_evaluated=False)
            with patch('sys.argv', argv), patch('experiments.k0_f2_interoception_confirmatory.protocol.load_lock', return_value=lock), \
                 patch('experiments.k0_f2_interoception_confirmatory.protocol.lock_sha256', return_value='f'*64), \
                 patch.object(probe, 'load_split', side_effect=[fixture_rows(), fixture_rows('validation', 'C')]), \
                 patch.object(probe, 'run_probe', return_value=(fixture_result, {}, [], [], [], [])) as runner:
                probe.main()
                runner.assert_called_once()
            self.assertEqual(val_path.read_bytes(), before)
            self.assertEqual(json.loads((output / 'probe_validation.json').read_text())['status'], 'FAIL_AT_PROBE_GATE')
            for name in probe.OUTPUT_ARTIFACTS:
                with self.subTest(artifact=name), tempfile.TemporaryDirectory() as protected:
                    owned = Path(protected) / name
                    owned.write_bytes(b'preserve existing artifact')
                    protected_argv = argv[:-1] + [protected]
                    with patch('sys.argv', protected_argv), patch('experiments.k0_f2_interoception_confirmatory.protocol.load_lock', return_value=lock), \
                         patch('experiments.k0_f2_interoception_confirmatory.protocol.lock_sha256', return_value='f'*64), \
                         patch.object(probe, 'load_split') as loader, patch.object(probe, 'run_probe') as runner:
                        with self.assertRaises(FileExistsError):
                            probe.main()
                        loader.assert_not_called()
                        runner.assert_not_called()
                    self.assertEqual(owned.read_bytes(), b'preserve existing artifact')


if __name__=='__main__':unittest.main()
