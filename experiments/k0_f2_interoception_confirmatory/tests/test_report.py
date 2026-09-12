import json
from pathlib import Path
import tempfile
import unittest
from experiments.k0_f2_interoception_confirmatory.report import generate,criterion,probe_numerical_gate


class ReportTest(unittest.TestCase):
    def setUp(self):self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name);self.out=self.root/'out'
    def tearDown(self):self.tmp.cleanup()
    def write(self,name,data):
        p=self.root/name;p.write_text(''.join(json.dumps(r)+'\n' for r in data) if p.suffix=='.jsonl' else json.dumps(data))
    def probe(self,passed=True):
        return dict(status='PASS' if passed else 'FAIL_AT_PROBE_GATE',gate={'pass':passed},rows=[dict(mode=m,target=t,split='validation',mae=.1 if m=='BODY' else .2,normalized_mae=.1 if m=='BODY' else .2,n_episodes=512,n_actions=4) for t in ('log_latency','utility') for m in ('BODY','BLIND','SHUFFLED','STALE')],selection_rows=[dict(mode=m,selection_regret=.1 if m=='BODY' else .2,accuracy=.9 if m=='BODY' else .8,n_episodes=512) for m in ('BODY','BLIND','SHUFFLED','STALE')],test_evaluated=False)
    def comparison(self,delta=.02):return dict(n_seeds=12,mean_difference=delta,mean=delta,sd=.001,median=delta,ci95=[delta-.002,delta+.002],exact_sign_p=.00048828125,**{'pass':True})
    def fixture(self):
        self.write('protocol_lock.json',dict(source_commit='fixed'))
        self.write('protocol_receipt.json',dict(protocol_lock_commit='locked',protocol_lock_sha256='identity'))
        self.write('normalization_config.json',dict(identity='fixed'))
        self.write('checkpoint_manifest.json',dict(status='complete'))
        self.write('collection_manifest.json',dict(sessions=[dict(session_id=s) for s in 'ABCD']))
        for name in ('raw_mac_telemetry.jsonl','raw_master_telemetry.jsonl','interoceptive_frames.jsonl'):
            self.write(name,[dict(timestamp=100+i,source_kind='real') for i in range(3)])
        self.write('probe_validation.json',self.probe())
        all_gates=('fresh_data','split_disjoint','normalization_train_only','leakage_free','checkpoint_identity','initial_weights_equal','seed_pairing','probe_recomputed','primary_recomputed','live_recomputed','counterfactual_recomputed','protocol_lock_valid')
        self.write('independent_audit.json',dict(gates={key:True for key in all_gates},**{'pass':True}))
        self.write('resource_summary.json',dict(safety_pass=True,execution_complete=True));self.write('final_runtime_state.json',dict(cleanup_pass=True))
        self.write('test_results.json',dict(rows=[dict(seed=s,architecture='GRU128',training_mode='BLIND' if m=='TRAINED_BLIND' else 'BODY',mode='BLIND' if m=='TRAINED_BLIND' else m,utility=.8 if m=='BODY' else .7) for s in range(12) for m in ('BODY','TRAINED_BLIND','SHUFFLED','STALE')],comparisons={key:self.comparison() for key in ('P1','P2','P3')}))
        self.write('live_results.json',dict(rows=[dict(seed=s,mode=m,utility=.8 if m=='BODY' else .7) for s in range(12) for m in ('BODY','TRAINED_BLIND')],comparisons={'P4':self.comparison()}))
        self.write('counterfactual_results.json',dict(rows=[dict(seed=s,available=True,action_change_rate=.2,utility_gain=.02) for s in range(12)],comparisons={'CF':self.comparison()}))
    def test_failed_gate_never_opens_downstream(self):
        self.write('probe_validation.json',self.probe(False))
        for name in ('training_config.json','run_summary.json','test_results.json','live_results.json','counterfactual_results.json','ood_results.json'):(self.root/name).write_text('DO NOT OPEN SEALED DATA')
        stats,result=generate(self.root,self.out)
        self.assertEqual(result['research_status'],'FAIL_AT_PROBE_GATE')
        self.assertFalse(stats['downstream_opened']);self.assertFalse(stats['errors'])
        self.assertNotIn('test_results.json',stats['files_opened'])
        report=(self.out/'K0_F2_REPORT.md').read_text();self.assertEqual(sum(line.startswith('## ') for line in report.splitlines()),27)
        self.assertIn('Session Dのtest',report);self.assertIn('未実施',report)
    def test_primary_requires_fixed_twelve_and_minimum_effect(self):
        value=self.comparison(.005);self.assertFalse(criterion(value))
        value=self.comparison();value['n_seeds']=8;self.assertFalse(criterion(value))
        value=self.comparison();value['n_seeds']=16;self.assertFalse(criterion(value))
    def test_probe_requires_both_targets_and_both_controls(self):
        p=self.probe();self.assertTrue(probe_numerical_gate(p)[0])
        p['rows'][0]['mae']=.4;self.assertFalse(probe_numerical_gate(p)[0])
        p=self.probe();p['selection_rows'][0]['selection_regret']=.3;self.assertFalse(probe_numerical_gate(p)[0])
    def test_saved_pass_does_not_override_failed_numeric_gate(self):
        self.fixture();p=self.probe();p['rows'][0]['mae']=.4;self.write('probe_validation.json',p)
        stats,result=generate(self.root,self.out);self.assertEqual(result['research_status'],'FAIL_AT_PROBE_GATE');self.assertFalse(stats['downstream_opened'])
    def test_all_gates_can_pass(self):
        self.fixture();_,result=generate(self.root,self.out);self.assertEqual(result['research_status'],'PASS')
    def test_masked_blind_not_primary_substitute(self):
        self.fixture();p=self.root/'test_results.json';v=json.loads(p.read_text());v['comparisons'].pop('P1');v['comparisons']['D_MASKED_BLIND']=self.comparison();self.write(p.name,v)
        _,result=generate(self.root,self.out);self.assertFalse(result['gates']['C_P1_independent_BLIND'])
    def test_live_failure_invalidates_replay_success(self):
        self.fixture();p=self.root/'live_results.json';v=json.loads(p.read_text());v['comparisons']['P4']=self.comparison(.005);self.write(p.name,v)
        _,result=generate(self.root,self.out);self.assertEqual(result['research_status'],'FAIL')
    def test_counterfactual_requires_point_zero_one_effect(self):
        self.fixture();p=self.root/'counterfactual_results.json';v=json.loads(p.read_text());v['comparisons']['CF']=self.comparison(.005);self.write(p.name,v)
        _,result=generate(self.root,self.out);self.assertFalse(result['gates']['F_counterfactual']);self.assertEqual(result['research_status'],'FAIL')
    def test_duplicate_seed_not_twelve(self):
        self.fixture();p=self.root/'test_results.json';v=json.loads(p.read_text());v['rows']=[dict(r,seed=0) for r in v['rows']];self.write(p.name,v)
        _,result=generate(self.root,self.out);self.assertFalse(result['gates']['C_P1_independent_BLIND'])
    def test_empty_is_incomplete_and_never_pass(self):
        stats,result=generate(self.root,self.out);self.assertEqual(result['research_status'],'INCOMPLETE');self.assertFalse(stats['downstream_opened'])


if __name__=='__main__':unittest.main()
