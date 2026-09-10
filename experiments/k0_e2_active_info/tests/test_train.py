import tempfile, unittest
from pathlib import Path
import torch
from experiments.k0_brainstem.models import make_model
from experiments.k0_e2_active_info.env import ActiveInfoEnv
from experiments.k0_e2_active_info.train import (rng_state,restore_rng,save_checkpoint,tensor_hash,critic_warmup,collect)

class TrainingContracts(unittest.TestCase):
    def setUp(self):torch.set_num_threads(2);torch.manual_seed(8)
    def test_checkpoint_restore_stage_and_rng(self):
        env=ActiveInfoEnv(8,'cpu',123,{'episode_length':8});env.reset()
        model=make_model('gru64');opt=torch.optim.Adam(model.parameters())
        config={'architecture':'gru64','seed':0}
        with tempfile.TemporaryDirectory() as d:
            for stage,update in [('initial',0),('imitation',64),('ppo',1)]:
                p=Path(d)/(stage+'.pt');save_checkpoint(p,model,opt,config,env,stage,update)
                expected=torch.rand(5);expectedobs=env.reset()
                cp=torch.load(p,weights_only=False);self.assertEqual(cp['stage'],stage);self.assertEqual(cp['update'],update)
                restored=make_model('gru64');restored.load_state_dict(cp['model']);self.assertEqual(tensor_hash(model),tensor_hash(restored))
                restore_rng(cp['rng'],env);self.assertTrue(torch.equal(torch.rand(5),expected));self.assertTrue(torch.equal(env.reset(),expectedobs))
    def test_critic_warmup_fixes_policy_and_restores_stream(self):
        env=ActiveInfoEnv(8,'cpu',200000,{'episode_length':8});env.reset();before=rng_state(env)
        model=make_model('gru64');weights={k:v.clone() for k,v in model.state_dict().items()}
        # Snapshot after model initialization; warmup must not change this global stream.
        before=rng_state(env);result=critic_warmup(model,env,{'seed':0,'minibatch_envs':4})
        after=rng_state(env);self.assertTrue(torch.equal(before['torch'],after['torch']))
        self.assertTrue(result['policy_unchanged'])
        for k,v in weights.items():
            if not k.startswith('critic.'):self.assertTrue(torch.equal(v,model.state_dict()[k]),k)
        self.assertTrue(any(not torch.equal(v,model.state_dict()[k]) for k,v in weights.items() if k.startswith('critic.')))
    def test_episode_collection_is_closed_loop(self):
        env=ActiveInfoEnv(8,'cpu',10,{'episode_length':8,'scenario':'language','language_latency':0})
        model=make_model('gru64');rollout,infos=collect(model,env,teacher_probability=1)
        self.assertTrue(bool(infos[0]['call_accepted'].all()))
        self.assertTrue(bool((rollout[0][1,:,10]>0).all()))

if __name__=='__main__':unittest.main()

class FactorComposition(unittest.TestCase):
    def test_composition_keeps_single_factor_exact(self):
        import copy
        from experiments.k0_e2_active_info.train import ppo_update
        torch.set_num_threads(2);torch.manual_seed(33)
        base=make_model('gru64');env=ActiveInfoEnv(8,'cpu',41,{'episode_length':8})
        rollout,_=collect(base,env)
        for arm,factors in [('B1',[]),('B5',['B5']),('B6',['B6']),('B7',['B7'])]:
            a,b=copy.deepcopy(base),copy.deepcopy(base);anchor=copy.deepcopy(base)
            oa,ob=torch.optim.Adam(a.parameters()),torch.optim.Adam(b.parameters())
            common={'ppo_epochs':2,'minibatch_envs':4}
            torch.manual_seed(100);la=ppo_update(a,oa,rollout,{**common,'arm':arm},anchor)
            torch.manual_seed(100);lb=ppo_update(b,ob,rollout,{**common,'arm':'B8','factors':factors},anchor)
            self.assertEqual(tensor_hash(a),tensor_hash(b));self.assertEqual(la,lb)
