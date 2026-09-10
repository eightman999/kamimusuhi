"""Virtual language and task oracle; no network or language model dependency."""
import torch
from .signals import INVOKE_LANGUAGE


class VirtualLanguageOracle:
    def __call__(self, required):
        return required.clone()


def reward_for(actions, targets, retention_mask, habituation_mask, llm_cost=.15):
    correct = actions == targets
    required = targets == INVOKE_LANGUAGE
    call = actions == INVOKE_LANGUAGE
    reward = torch.where(correct, 1., -.3)
    reward = torch.where(required & ~call, -1., reward)
    reward = torch.where(~required & call, -float(llm_cost), reward)
    # Language calls consume resources even when required. Missing a required
    # call remains worse at normal costs. This yields a controllable Pareto tradeoff.
    reward = torch.where(required & call, reward - float(llm_cost), reward)
    reward += correct * retention_mask * .1 + correct * habituation_mask * .05
    return reward, correct, required
