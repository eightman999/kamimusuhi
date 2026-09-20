"""Synthetic live protocol boundaries; these do not execute hardware jobs."""
import copy
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from experiments.k0_f2_interoception_confirmatory import live


def fixture():
    rows=[]
    for sid in ('L1','L2'):
        for block in range(8):
            for seed in range(12):
                for task in range(4):
                    for order,mode in enumerate(live.MODES):
                        latency=.002 if mode=='BODY' else .004
                        rows.append(dict(session_id=sid,block_id=f'{sid}-{block}',seed=seed,task_id=task,
                            mode=mode,order_index=order,utility=1-latency/.01,success=True,action=1,
                            measurement=dict(latency_seconds=latency,ok=True,finite=True)))
    return rows


class LiveTests(unittest.TestCase):
    def test_fixed_complete_pairing_and_primary(self):
        rows=fixture();result=live.aggregate(rows)
        self.assertEqual(len(result['rows']),48)
        self.assertTrue(all(r['n_jobs']==64 for r in result['rows']))
        self.assertTrue(result['comparisons']['P4']['pass'])
        self.assertAlmostEqual(result['comparisons']['P4']['mean'],.2)
        self.assertEqual(live.order_effect(rows)['rows'][0]['n_jobs'],768)
        with self.assertRaises(ValueError):live.aggregate(rows[:-1])
        with self.assertRaises(ValueError):live.aggregate(rows+rows[:1])
        with self.assertRaises(ValueError):live.aggregate([r for r in rows if r['session_id']=='L1'])

    def test_live_guard_blocks_all_models_and_test_reads(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);(root/'validation').mkdir();(root/'train').mkdir()
            (root/'validation/probe_validation.json').write_text(json.dumps({'gate':{'pass':False},'protocol_lock_sha256':'a'}))
            (root/'train/checkpoint_manifest.json').write_text(json.dumps({'complete':False,'protocol_lock_sha256':'a'}))
            lock={'live':{'sessions':{'L1':{'seed':1}},'blocks_per_session':8,'block_max_seconds':120}}
            args=SimpleNamespace(protocol_lock=root/'lock',session_id='L1',blocks=8,block_seconds=120,study_root=root,training_artifacts=root/'train')
            with patch.object(live,'load_lock',return_value=lock),patch.object(live,'lock_sha256',return_value='a'),patch.object(live,'lock_receipt',return_value={}),patch.object(live,'load_split') as read,patch.object(live,'load_model') as model:
                with self.assertRaises(RuntimeError):live.collect(args)
                read.assert_not_called();model.assert_not_called()
            args=SimpleNamespace(protocol_lock=root/'lock',study_root=root)
            with patch.object(live,'load_lock',return_value=lock),patch.object(live,'lock_sha256',return_value='a'):
                with self.assertRaises(RuntimeError):live.combine(args)
            self.assertFalse((root/'live').exists())

if __name__=='__main__':unittest.main()
