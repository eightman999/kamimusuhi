import json
import numpy as np
import pytest
import torch
from experiments.k0_e2_active_info.evaluate import (
    evaluate_episode, matched_schedule, counterfactual, ambiguity_audit,
    action_metrics, seed_statistics, paired_sign_test, evaluate_checkpoint,
    transform_observation, OOD_MODES)


class InformationPolicy(torch.nn.Module):
    """Fixture policy consumes acquired information; never reads an env latent."""
    def initial_state(self,batch,device):
        return torch.full((batch,1),.5,device=device)
    def forward(self,obs,state):
        next_state=torch.where((obs[:,10]>.5)[:,None],obs[:,9:10],state)
        answer=torch.where(next_state[:,0]>.5,4,3)
        action=torch.where((obs[:,2]>.5)&(obs[:,7]>.5)&(obs[:,10]==0),5,0)
        action=torch.where(obs[:,14]>.5,answer,action)
        logits=torch.zeros(obs.shape[0],6,device=obs.device)
        logits.scatter_(1,action[:,None],10.)
        return logits,torch.zeros(obs.shape[0],device=obs.device),next_state


@pytest.fixture(autouse=True)
def cpu_threads():
    old=torch.get_num_threads();torch.set_num_threads(4)
    yield
    torch.set_num_threads(old)


def test_language_information_response_and_causal_fork():
    policy=InformationPolicy();cfg={'scenario':'language','episode_length':8,'language_latency':0}
    correct,_=evaluate_episode(policy,cfg,n_envs=128)
    inverted,_=evaluate_episode(policy,cfg,n_envs=128,response_mode='inverted')
    missing,_=evaluate_episode(policy,cfg,n_envs=128,response_mode='missing')
    assert correct['task_success']==1 and inverted['task_success']==0
    assert .35<missing['task_success']<.65
    causal=counterfactual(policy,cfg,n_envs=128)
    again=counterfactual(policy,cfg,n_envs=128)
    assert causal['delta_success']>.35
    assert causal['delta_downstream_action_rate']>0
    assert causal['paired_episode_delta_reward']==again['paired_episode_delta_reward']
    assert causal['call']['accepted_calls']==128 and causal['no_call']['accepted_calls']==0


def test_matched_random_actual_accepted_count_and_no_language_ambiguity():
    model=InformationPolicy();cfg={'episode_length':8}
    learned,trace=evaluate_episode(model,cfg,n_envs=96)
    schedule=matched_schedule(trace,99)
    assert int(schedule.sum())==learned['accepted_calls']
    randomized,_=evaluate_episode(model,cfg,n_envs=96,gate='matched_rate_random',call_schedule=schedule)
    assert randomized['accepted_calls']==learned['accepted_calls']
    audit=ambiguity_audit(model,cfg,n_envs=32)
    assert audit['passed'] and audit['balanced_latent_success']==.5


def test_strict_memory_reset_and_reproducibility():
    model=InformationPolicy();cfg={'scenario':'memory','memory_delay':24,'strict_memory':True}
    normal,t=evaluate_episode(model,cfg,n_envs=128)
    reset,_=evaluate_episode(model,cfg,n_envs=128,state_mode='reset_step')
    _,again=evaluate_episode(model,cfg,n_envs=128)
    assert normal['memory_retention']==1
    assert .35<reset['memory_retention']<.65
    assert torch.equal(t['rewards'],again['rewards'])


def test_confusion_and_seed_units():
    actions=torch.tensor([[0,1,1],[5,5,4]])
    reference=torch.tensor([[0,1,2],[5,4,4]])
    metrics=action_metrics(actions,reference)
    assert sum(map(sum,metrics['confusion_matrix']))==6
    assert metrics['per_action_precision'][1]==.5
    assert metrics['per_action_recall'][4]==.5
    stats=seed_statistics([1,2,3])
    assert stats['n_seeds']==3 and stats['mean']==2 and stats['sd']==1
    assert paired_sign_test([1]*8)['two_sided_exact_p']==2/256


def test_shortcut_preserves_public_protocol_except_explicit_mask():
    obs=torch.arange(32).reshape(2,16).float()
    generator=torch.Generator().manual_seed(1)
    transformed=transform_observation(obs,'channel_shuffle',0,8,generator,torch.arange(10,-1,-1),[])
    assert torch.equal(transformed[:,11:],obs[:,11:])
    assert not torch.equal(transformed[:,:11],obs[:,:11])


def test_all_ood_modes_are_supported_and_metrics_finite():
    for mode in OOD_MODES:
        metrics,_=evaluate_episode(InformationPolicy(),{'episode_length':8,'ood':mode},n_envs=12)
        assert np.isfinite(metrics['reward'])
        assert 0<=metrics['task_success']<=1


def test_evaluation_identity_rejects_stale_settings(tmp_path):
    from experiments.k0_brainstem.models import make_model
    run=tmp_path/'runs'/'gru64-s0-B0';run.mkdir(parents=True)
    checkpoint=run/'imitation_best.pt'
    model=make_model('gru64')
    torch.save({'model':model.state_dict(),'config':{'architecture':'gru64','seed':0,'env':{'episode_length':8}},'stage':'imitation','update':1},checkpoint)
    result=evaluate_checkpoint(checkpoint,tmp_path/'out',n_envs=8,long_envs=8,basic_only=True)
    assert result['cpu_benchmark_batch']==1 and result['cpu_benchmark_threads']==4
    assert result['cpu_inference_ms_p95']>0
    with pytest.raises(ValueError,match='identity changed'):
        evaluate_checkpoint(checkpoint,tmp_path/'out',n_envs=16,long_envs=8,basic_only=True)


def test_finite_matrix_incremental_outputs_and_ten_plots(tmp_path, monkeypatch):
    import experiments.k0_e2_active_info.evaluate as module
    from experiments.k0_brainstem.models import make_model
    from experiments.k0_e2_active_info.visualize import render
    monkeypatch.setattr(module,'COSTS',[0,.4,1.2])
    monkeypatch.setattr(module,'RELIABILITIES',[1.,.5])
    monkeypatch.setattr(module,'LATENCIES',[0,2])
    monkeypatch.setattr(module,'DELAYS',[8,16])
    run=tmp_path/'runs'/'gru64-s0-B0';run.mkdir(parents=True)
    checkpoint=run/'imitation_best.pt';model=make_model('gru64')
    torch.save({'model':model.state_dict(),'config':{'architecture':'gru64','seed':0,'env':{'episode_length':8}},'stage':'imitation','update':1},checkpoint)
    module.evaluate_checkpoint(checkpoint,tmp_path,n_envs=8,long_envs=8)
    directory=tmp_path/'evaluations'/'gru64-s0-B0__imitation_best'
    assert json.loads((directory/'complete.json').read_text())['complete']
    for filename in ['voi_results','policy_interventions','language_response_ablation','retention_results','ood_results','counterfactual','shortcut_audit','active_sensing','habituation_results','ambiguity_audit']:
        assert json.loads((directory/(filename+'.json')).read_text())['complete']
    module.consolidate(tmp_path)
    manifest=render(tmp_path)
    assert manifest['plot_count']==10
    assert all((tmp_path/name).stat().st_size>1000 for name in manifest['files'])
