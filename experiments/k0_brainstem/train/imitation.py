"""Full sequence imitation, balanced actions learned from oracle demonstrations."""
import torch
import torch.nn.functional as F


def update(model, optimizer, obs, targets, config, infos=None):
    losses=[]
    counts=torch.bincount(targets.flatten(),minlength=6).float().clamp_min(1)
    weights=(counts.sum()/counts).sqrt(); weights/=weights.mean()
    for ids in torch.randperm(obs.shape[1],device=obs.device).split(config.get('minibatch_envs',256)):
        logits,_,_=model.forward_sequence(obs[:,ids],model.initial_state(len(ids),obs.device))
        losses_per=F.cross_entropy(logits.flatten(0,1),targets[:,ids].flatten(),weight=weights,reduction='none').reshape(len(obs),len(ids))
        importance=torch.ones_like(losses_per)
        if infos is not None:
            importance+=torch.stack([i['retention_mask'][ids] for i in infos]).float()*31
            importance+=torch.stack([i['novelty_mask'][ids] for i in infos]).float()*3
        loss=(losses_per*importance).sum()/importance.sum()
        optimizer.zero_grad(set_to_none=True); loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(),1.0); optimizer.step()
        losses.append(float(loss.detach()))
    return sum(losses)/max(len(losses),1)
