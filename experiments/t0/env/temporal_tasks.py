"""Auxiliary temporal tasks T0-B/C/D built on BaseTemporalEnv machinery.

T0-B IntervalComparisonEnv: two marker intervals; answer which was longer.
T0-C TemporalOrderEnv:      pulses A and B at random times; answer which was first.
T0-D DelayedResponseEnv:    binary cue at t=0; answer with the right action
                            inside the window T* = round(D / time_scale).

All three use a 3-action commit protocol (WAIT plus two answer actions) and the
same stochastic dynamics/distractor channels as IntervalEnv.  No channel ever
carries timestep, absolute/remaining time, progress, or position encodings.
"""
import torch

from .interval_env import (BaseTemporalEnv, OBS_DIM, OBS_NAMES, SEEN_DELAYS)

WAIT = 0
A_FIRST, B_FIRST = 1, 2       # T0-C answers
LONGER_FIRST, LONGER_SECOND = 1, 2   # T0-B answers
ANSWER_A, ANSWER_B = 1, 2     # T0-D answers


class IntervalComparisonEnv(BaseTemporalEnv):
    """Four identical marker pulses: 0, i1, i1+g, i1+g+i2.  Compare i1 vs i2."""

    ACTION_NAMES = ("WAIT", "LONGER_FIRST", "LONGER_SECOND")

    def reset(self):
        n = self.num_envs
        self.t = 0
        lo = int(self.config.get("interval_min", 4))
        hi = int(self.config.get("interval_max", self.horizon // 4))
        gap = int(self.config.get("interval_gap", 8))
        i1 = self._randint(hi - lo + 1, (n,)) + lo
        i2 = self._randint(hi - lo + 1, (n,)) + lo
        equal = i1 == i2
        while bool(equal.any()):
            i2 = torch.where(equal, self._randint(hi - lo + 1, (n,)) + lo, i2)
            equal = i1 == i2
        self.interval1, self.interval2 = i1, i2
        self.pulse_steps = torch.stack([
            torch.zeros(n, dtype=torch.long, device=self.device),
            i1, i1 + gap, i1 + gap + i2])
        self.answer = (i2 > i1).long() + 1   # 1: first longer, 2: second longer
        self._init_streams()
        self.answered = torch.zeros(n, dtype=torch.bool, device=self.device)
        self.correct = torch.zeros_like(self.answered)
        self._initialized = True
        self.observation = self._current_observation()
        return self.observation.clone()

    def _current_observation(self):
        obs = self._base_observation()
        obs[:, 0] = (self.pulse_steps == self.t).any(0).float()
        obs[:, 1] = 0.
        return obs.clamp_(0, 1)

    def oracle_actions(self):
        return torch.where(self.t >= self.pulse_steps[3],
                           self.answer, torch.zeros_like(self.answer))

    def step(self, actions):
        if not self._initialized or self.t >= self.horizon:
            raise RuntimeError("reset required")
        actions = torch.as_tensor(actions, device=self.device, dtype=torch.long)
        committing = (actions > 0) & ~self.answered
        reward = torch.zeros(self.num_envs, device=self.device)
        reward = torch.where(committing,
                             torch.where(actions == self.answer, 1., -1.), reward)
        self.answered |= committing
        self.correct |= committing & (actions == self.answer)
        self.t += 1
        done = self.answered | (self.t >= self.horizon)
        reward = torch.where((self.t >= self.horizon) & ~self.answered,
                             torch.full_like(reward, -1.), reward)
        info = {"success": self.correct.clone(), "answered": self.answered.clone(),
                "interval1": self.interval1.clone(), "interval2": self.interval2.clone(),
                "step": self.t}
        self.observation = self._current_observation() if self.t < self.horizon \
            else torch.zeros(self.num_envs, OBS_DIM, device=self.device)
        return self.observation.clone(), reward, done, info


class TemporalOrderEnv(BaseTemporalEnv):
    """Pulse on channel A then B (or reverse) at random times; which was first?"""

    ACTION_NAMES = ("WAIT", "A_FIRST", "B_FIRST")

    def reset(self):
        n = self.num_envs
        self.t = 0
        hi = max(4, self.horizon // 2)
        self.time_a = self._randint(hi, (n,))
        self.time_b = self._randint(hi, (n,))
        equal = self.time_a == self.time_b
        while bool(equal.any()):
            self.time_b = torch.where(equal, self._randint(hi, (n,)), self.time_b)
            equal = self.time_a == self.time_b
        self.answer = (self.time_b < self.time_a).long() + 1  # 1: A first, 2: B first
        self._init_streams()
        self.answered = torch.zeros(n, dtype=torch.bool, device=self.device)
        self.correct = torch.zeros_like(self.answered)
        self._initialized = True
        self.observation = self._current_observation()
        return self.observation.clone()

    def _current_observation(self):
        obs = self._base_observation()
        obs[:, 0] = (self.time_a == self.t).float()
        obs[:, 1] = (self.time_b == self.t).float()
        return obs.clamp_(0, 1)

    def oracle_actions(self):
        both = (self.t >= self.time_a) & (self.t >= self.time_b)
        return torch.where(both, self.answer, torch.zeros_like(self.answer))

    def step(self, actions):
        if not self._initialized or self.t >= self.horizon:
            raise RuntimeError("reset required")
        actions = torch.as_tensor(actions, device=self.device, dtype=torch.long)
        committing = (actions > 0) & ~self.answered
        reward = torch.where(committing,
                             torch.where(actions == self.answer, 1., -1.),
                             torch.zeros(self.num_envs, device=self.device))
        self.answered |= committing
        self.correct |= committing & (actions == self.answer)
        self.t += 1
        done = self.answered | (self.t >= self.horizon)
        reward = torch.where((self.t >= self.horizon) & ~self.answered,
                             torch.full_like(reward, -1.), reward)
        info = {"success": self.correct.clone(), "answered": self.answered.clone(),
                "time_a": self.time_a.clone(), "time_b": self.time_b.clone(), "step": self.t}
        self.observation = self._current_observation() if self.t < self.horizon \
            else torch.zeros(self.num_envs, OBS_DIM, device=self.device)
        return self.observation.clone(), reward, done, info


class DelayedResponseEnv(BaseTemporalEnv):
    """Binary cue at t=0, delay D, then answer A/B inside the window."""

    ACTION_NAMES = ("WAIT", "ANSWER_A", "ANSWER_B")

    def __init__(self, num_envs, device="cpu", seed=0, config=None):
        super().__init__(num_envs, device, seed, config)
        self.window = int(self.config.get("window", 2))
        self.delay_scale = float(self.config.get("delay_scale", 128.))

    def reset(self, delay_override=None):
        n = self.num_envs
        self.t = 0
        grid = list(self.config.get("delays", SEEN_DELAYS))
        values = torch.tensor(grid, device=self.device)
        self.delay = values[self._randint(len(grid), (n,))].long()
        if delay_override is not None:
            supplied = torch.as_tensor(delay_override, device=self.device, dtype=torch.long)
            if supplied.numel() == 1:
                supplied = supplied.expand(n)
            self.delay = supplied.clone()
        self.target_step = torch.round(self.delay.float() / self.time_scale).long()
        self.target_step = self.target_step.clamp(self.pulse_length, self.horizon - 1)
        self.cue = self._randint(2, (n,))          # 0 -> ANSWER_A, 1 -> ANSWER_B
        self._init_streams()
        self.answered = torch.zeros(n, dtype=torch.bool, device=self.device)
        self.answer_step = torch.full((n,), -1, dtype=torch.long, device=self.device)
        self.correct = torch.zeros_like(self.answered)
        self._initialized = True
        self.observation = self._current_observation()
        return self.observation.clone()

    def _current_observation(self):
        obs = self._base_observation()
        pulsing = self.t < self.pulse_length
        obs[:, 0] = float(pulsing)
        obs[:, 1] = (.25 + .5 * self.cue.float()) if pulsing else 0.
        return obs.clamp_(0, 1)

    def oracle_actions(self):
        due = self.t >= self.target_step
        return torch.where(due, self.cue + 1, torch.zeros_like(self.cue))

    def step(self, actions):
        if not self._initialized or self.t >= self.horizon:
            raise RuntimeError("reset required")
        actions = torch.as_tensor(actions, device=self.device, dtype=torch.long)
        committing = (actions > 0) & ~self.answered
        on_time = (self.t - self.target_step).abs() <= self.window
        right = actions == self.cue + 1
        reward = torch.where(committing,
                             torch.where(right & on_time, 1., -1.),
                             torch.zeros(self.num_envs, device=self.device))
        self.answered |= committing
        self.answer_step = torch.where(committing, torch.full_like(self.answer_step, self.t),
                                       self.answer_step)
        self.correct |= committing & right & on_time
        self.t += 1
        done = self.answered | (self.t >= self.horizon)
        reward = torch.where((self.t >= self.horizon) & ~self.answered,
                             torch.full_like(reward, -1.), reward)
        info = {"success": self.correct.clone(), "answered": self.answered.clone(),
                "answer_step": self.answer_step.clone(), "delay": self.delay.clone(),
                "target_step": self.target_step.clone(), "cue": self.cue.clone(),
                "step": self.t}
        self.observation = self._current_observation() if self.t < self.horizon \
            else torch.zeros(self.num_envs, OBS_DIM, device=self.device)
        return self.observation.clone(), reward, done, info


def make_task(name, num_envs, device="cpu", seed=0, config=None):
    from .interval_env import IntervalEnv
    tasks = {"interval": IntervalEnv, "comparison": IntervalComparisonEnv,
             "order": TemporalOrderEnv, "delayed_response": DelayedResponseEnv}
    if name not in tasks:
        raise ValueError(f"Unknown task: {name}")
    return tasks[name](num_envs, device, seed, config)
