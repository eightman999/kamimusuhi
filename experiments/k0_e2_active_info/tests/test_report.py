import json
import math
import re
import tempfile
import shutil
from pathlib import Path
import pytest
from experiments.k0_e2_active_info.report import generate, positive_contrast, PLOTS, REQUIRED
ROOT=Path(__file__).resolve().parents[1]/'artifacts'/'primary'
def pair(deltas):
    return positive_contrast([dict(seed=i,score=v) for i,v in enumerate(deltas)],[dict(seed=i,score=0) for i in range(len(deltas))],'score')
def test_direction_and_independent_seed_count():
    assert pair([1]*8)['status']=='PASS'
    assert pair([-1]*8)['status']=='FAIL'
    assert pair([0]*8)['status']=='FAIL'
    assert pair([-1,1]*4)['status']=='PARTIAL'
    assert pair([1]*7)['status']=='PARTIAL'
    a=[dict(seed=i,score=i) for i in range(8)]
    b=list(reversed([dict(seed=i,score=i-1) for i in range(8)]))
    c=positive_contrast(a,b,'score')
    assert c['paired_differences']==[1]*8
    assert c['sign_test']['two_sided_exact_p']==.0078125

def test_missing_evidence_is_not_success(tmp_path):
    result=generate(tmp_path)
    assert not result['execution_complete']
    assert set(result['missing_inputs'])==set(REQUIRED)
    assert result['research_status']=='PARTIAL'
    with pytest.raises(RuntimeError): generate(tmp_path,strict=True)

def test_actual_report_recomputed_from_saved_primary(tmp_path):
    source=ROOT
    if not (source/'run_summary.json').exists():pytest.skip('requires saved primary experimental results')
    for p in source.iterdir():
        if p.is_file() and p.suffix in ('.json','.png'):shutil.copyfile(p,tmp_path/p.name)
    for p in (source/'runs').glob('*/*.json'):
        target=tmp_path/p.relative_to(source);target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,target)
    return check_actual_report(tmp_path)

def check_actual_report(ROOT):
    result=generate(ROOT,strict=True)
    assert result['execution_complete'] and result['research_status']=='FAIL'
    rows=json.loads((ROOT/'run_summary.json').read_text())
    stats=json.loads((ROOT/'report_statistics.json').read_text())
    assert len(rows)==112
    for arch in ['mlp','gru64','gru128']:
        primary=[r for r in rows if r['architecture']==arch and r['run_id'].endswith('-B0') and r['checkpoint_name']=='imitation_best.pt']
        assert len(primary)==8
        assert stats['architecture'][arch]['task_success']['n_seeds']==8
        assert math.isclose(stats['architecture'][arch]['task_success']['mean'],sum(r['task_success'] for r in primary)/8)
    assert result['conditions']['recurrent_memory_dependency']['status']=='PASS'
    assert result['conditions']['memory_temporal_ood_generalization']['status']=='FAIL'
    assert result['conditions']['j72_closed_loop']['status']=='FAIL'
    assert result['combined_B8_complete']
    assert result['category_coverage']['voi_results.json']['expected_conditions_per_seed']==19
    assert result['category_coverage']['voi_results.json']['record_count']==456
    assert stats['trained_memory_delay47']['gru128']['statistics']['mean']==.546875
    assert stats['ppo']['B8']['vs_B0']['statistics']['mean']==0
    text=(ROOT/'K0_E2_REPORT.md').read_text()
    assert len(re.findall(r'^## ',text,re.M))==21
    assert all(f']({p})' in text for p in PLOTS)
    assert stats['selected_architecture']=='gru128'
    assert '実parameter総数=150001152' in text
    assert '観測上は変化なし・性能保持' in text
    assert '--checkpoint \"$cp\" --basic-only' in text
    assert 'CIは訓練seed変動だけ' in text
    assert 'memory horizonや50% crossingは定義しない' in text
    assert json.loads((ROOT/'success_conditions.json').read_text())==result
