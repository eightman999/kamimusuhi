import json
import numpy as np
import pytest
from experiments.g0_v5.src.controls import PCAControl, temporal_readout, raw_encode
from experiments.g0_v5.src.protocol import freeze, verify_lock, same_metrics, pilot_gate


def test_pca_is_train_only_and_readout_finite():
    rng=np.random.default_rng(3)
    train=rng.normal(size=(16,16,24)).astype('float32')
    val=rng.normal(size=(8,16,24)).astype('float32')
    p=PCAControl(train,16); before=p.components.copy(); p(val*100)
    np.testing.assert_array_equal(before,p.components)
    assert np.isfinite(temporal_readout(raw_encode,train,val)['mse'])


def test_lock_rejects_source_and_manifest_mutations(tmp_path):
    source=tmp_path/'src'; source.mkdir(); (source/'a.py').write_text('x=1\n')
    data=tmp_path/'dataset.json';data.write_text('{}')
    lock=tmp_path/'lock.json'; freeze(source,lock,data);verify_lock(source,lock,data)
    (source/'a.py').write_text('x=2\n')
    with pytest.raises(RuntimeError):verify_lock(source,lock,data)


def test_gate_rejects_rank_collapse_and_no_learning():
    result={'collapse':{'full_collapse':False,'dimensional_collapse':True,'shortcut_context':False},'failure_class':'COLLAPSE_DIM'}
    g=pilot_gate(result,result,{'mse':.5},{'mse':1.},True,True,True)
    assert not g['admitted']
    result['collapse']['dimensional_collapse']=False;result['failure_class']='NONE'
    assert not pilot_gate(result,result,{'mse':1.},{'mse':1.},True,True,True)['admitted']
    assert pilot_gate(result,result,{'mse':.5},{'mse':1.},True,True,True)['admitted']


def test_repeat_scientific_fields_not_runtime():
    assert same_metrics({'auc':.6,'runtime':{'seconds':1}}, {'auc':.6,'runtime':{'seconds':3}})
    assert not same_metrics({'auc':.6},{'auc':.7})
    assert not same_metrics({'auc':float('nan')},{'auc':float('nan')})
