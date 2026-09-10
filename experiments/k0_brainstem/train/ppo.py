"""PPO on whole recurrent sequences; never shuffle individual timesteps."""
import torch


def advantages(rewards, values, gamma=.99, lam=.95):
    result = torch.zeros_like(rewards)
    carry = torch.zeros_like(rewards[0])
    for t in reversed(range(len(rewards))):
        following = values[t+1] if t+1 < len(values) else torch.zeros_like(carry)
        delta = rewards[t] + gamma * following - values[t]
        carry = delta + gamma * lam * carry
        result[t] = carry
    return result, result + values


def update(model, optimizer, rollout, config):
    obs, acts, old_log, old_values, rewards = rollout
    adv, returns = advantages(rewards, old_values, config.get('gamma', .99), config.get('gae_lambda', .95))
    adv = (adv - adv.mean()) / (adv.std() + 1e-8)
    losses=[]
    for _ in range(config.get('ppo_epochs', 2)):
        order=torch.randperm(obs.shape[1], device=obs.device)
        for ids in order.split(config.get('minibatch_envs', 256)):
            logits, values, _ = model.forward_sequence(obs[:,ids],model.initial_state(len(ids),obs.device))
            dist=torch.distributions.Categorical(logits=logits)
            ratio=(dist.log_prob(acts[:,ids])-old_log[:,ids]).exp()
            a=adv[:,ids]
            c=config.get('clip_ratio',.2)
            policy=-torch.minimum(ratio*a, ratio.clamp(1-c,1+c)*a).mean()
            loss=policy+.5*(values-returns[:,ids]).square().mean()-.01*dist.entropy().mean()
            optimizer.zero_grad(set_to_none=True); loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(),1.0); optimizer.step()
            losses.append(float(loss.detach()))
    return sum(losses)/max(len(losses),1)
