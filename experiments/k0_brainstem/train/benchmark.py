"""Bounded end-to-end update throughput pilot; not a completed training run."""
import json,time,argparse
import torch
from ..models import make_model
from ..env.vector_env import VectorEnv
from .trainer import collect
from .imitation import update

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--device',default='cuda:0');a=p.parse_args();torch.set_num_threads(4);rows=[]
    for n in [8192,16384]:
        torch.manual_seed(700);model=make_model('gru64').to(a.device);opt=torch.optim.Adam(model.parameters(),lr=.001)
        env=VectorEnv(n,128,a.device,700)
        for repeat in range(2):
            torch.cuda.reset_peak_memory_stats(a.device);torch.cuda.synchronize(a.device);start=time.perf_counter()
            roll,targets,infos,_=collect(model,env,a.device,True)
            loss=update(model,opt,roll[0],targets,{'minibatch_envs':256},infos)
            torch.cuda.synchronize(a.device);elapsed=time.perf_counter()-start
            rows.append({'device':torch.cuda.get_device_name(a.device),'envs':n,'repeat':repeat,'steps_per_second':n*128/elapsed,'elapsed':elapsed,'peak_vram_mb':torch.cuda.max_memory_allocated(a.device)/2**20})
        del model,opt,env,roll,targets,infos;torch.cuda.empty_cache()
    print(json.dumps(rows,indent=2))
