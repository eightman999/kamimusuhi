import copy,json,tempfile,unittest
from pathlib import Path
from unittest.mock import patch
from experiments.k0_f_interoception.prepare_dataset import prepare
from experiments.k0_f_interoception.acquire import Background

class AcquisitionTests(unittest.TestCase):
    def test_existing_scratch_is_preserved(self):
        with tempfile.TemporaryDirectory() as d:
            out=Path(d);(out/'scratch').mkdir();p=out/'scratch/k0f-fixture.bin';p.write_bytes(b'existing-user-data')
            with patch('experiments.k0_f_interoception.acquire.devices',return_value=['cpu','cuda:0','cuda:1']):
                with self.assertRaises(FileExistsError):Background('disk_io',1,out,'fixture')
            self.assertEqual(p.read_bytes(),b'existing-user-data')

    def test_balanced_preparation_uses_only_past_and_preserves_source(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);frames=[]
            for i in range(160):frames.append(dict(frame_id=f'f-{i}',timestamp=float(i),values=[i/200]*18+[0.,1.],mask=[1.]*20,normalization_identity='fixture'))
            fp=p/'frames.jsonl';fp.write_text(''.join(json.dumps(r)+'\n' for r in frames))
            rows=[]
            for b,split in enumerate(['train','validation','test']):
                for i in range(8):rows.append(dict(episode_id=f'{b}-{i}',block_id=f'b-{b}',split=split,timestamp=50.+b*30+i,costs_seconds=[1.,2.,3.],provenance={},task_features=[0.,0.,0.,0.]))
            raw=p/'raw.jsonl';raw.write_text(''.join(json.dumps(r)+'\n' for r in rows));original=raw.read_bytes()
            result=p/'prepared.jsonl';prepare(raw,fp,result);a=[json.loads(s) for s in result.read_text().splitlines()]
            self.assertEqual(raw.read_bytes(),original)
            for block in range(3):self.assertEqual(sorted(len(r['body_sequence']) for r in a if r['block_id']==f'b-{block}'),list(range(1,9)))
            for row in a:
                self.assertTrue(all(float(i.split('-')[1])<=row['timestamp'] for i in row['provenance']['frame_ids']))
                self.assertTrue(all(float(i.split('-')[1])<=row['timestamp']-30 for i in row['provenance']['stale_frame_ids']))
                self.assertGreaterEqual(row['stale_body_sequence'][-1][18],.5)
            alternate=copy.deepcopy(rows)
            for row in alternate:row['costs_seconds']=[300.,200.,100.]
            raw2=p/'alternate.jsonl';raw2.write_text(''.join(json.dumps(r)+'\n' for r in alternate));out2=p/'alternate-prepared.jsonl';prepare(raw2,fp,out2)
            b=[json.loads(s) for s in out2.read_text().splitlines()]
            self.assertEqual([r['body_sequence'] for r in a],[r['body_sequence'] for r in b])
            with self.assertRaises(FileExistsError):prepare(raw,fp,result)

if __name__=='__main__':unittest.main()
