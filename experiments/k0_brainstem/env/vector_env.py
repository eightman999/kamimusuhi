"""Synchronous vectorized tensor environment; reset explicitly after done."""
import torch
from .scenario_generator import generate
from .oracle import reward_for


class VectorEnv:
    def __init__(self, num_envs, episode_length=128, device="cpu", seed=0, ood=None):
        self.num_envs = int(num_envs)
        self.episode_length = int(episode_length)
        self.device = torch.device(device)
        self.generator = torch.Generator(device=self.device).manual_seed(seed)
        self.ood = ood
        self.llm_cost = .15
        self._time = 0
        self._data = None

    @property
    def scenario(self):
        return self._data["scenario"]

    def reset(self, cue_override=None):
        self._data = generate(self.num_envs, self.episode_length, self.device, self.generator, self.ood, cue_override)
        self._time = 0
        return self._data["observations"][0]

    def oracle_actions(self):
        return self._data["targets"][self._time]

    def step(self, actions):
        if self._data is None or self._time >= self.episode_length:
            raise RuntimeError("reset() required before stepping")
        if actions.shape != (self.num_envs,):
            raise ValueError("actions must have shape [num_envs]")
        t = self._time
        targets = self.oracle_actions()
        retention = self._data["retention_mask"][t]
        habituation = self._data["habituation_mask"][t]
        reward, correct, required = reward_for(actions, targets, retention, habituation, self.llm_cost)
        info = dict(oracle_action=targets, scenario=self.scenario,
                    required_llm=required, retention_mask=retention,
                    habituation_mask=habituation,
                    novelty_mask=self._data["novelty_mask"][t],
                    first_stimulus_mask=self._data["first_stimulus_mask"][t],
                    renewed_stimulus_mask=self._data["renewed_stimulus_mask"][t],
                    success=correct)
        self._time += 1
        terminal = self._time == self.episode_length
        done = torch.full((self.num_envs,), terminal, dtype=torch.bool, device=self.device)
        observation = self._data["observations"][min(self._time, self.episode_length - 1)]
        return observation, reward, done, info
