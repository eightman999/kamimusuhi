"""Batched numeric interval-production environment (T0-A).

No channel ever carries the current timestep, absolute time, remaining time,
normalized progress, or a deterministic position encoding derived from
environment time.  The only time-bearing signals are stochastic world-dynamics
channels (a random-phase oscillator and a mean-reverting random walk) whose
rate follows ``time_scale``; they carry *state change* information, not a
clock, and they differ across episodes at every fixed step.

Task: a stimulus pulse at t=0 encodes a target delay D on the cue channel
(intensity = D / delay_scale).  The agent must commit ACT at step
T* = round(D / time_scale).  |t_act - T*| <= window is a success.
"""
import torch

WAIT, EXPECT, ACT = range(3)
ACTION_NAMES = ("WAIT", "EXPECT", "ACT")
OBS_NAMES = ("stimulus_pulse", "cue_intensity", "oscillator", "ou_walk",
             "distractor", "noise_a", "noise_b", "noise_c")
OBS_DIM = len(OBS_NAMES)
NUM_ACTIONS = len(ACTION_NAMES)

SEEN_DELAYS = (8, 16, 24, 32, 48, 64)
INTERP_DELAYS = (40, 56)
EXTRAP_DELAYS = (80, 96, 128)


class BaseTemporalEnv:
    """Shared batched-tensor machinery: RNG, dynamics channels, snapshot."""

    def __init__(self, num_envs, device="cpu", seed=0, config=None):
        self.num_envs = int(num_envs)
        self.device = torch.device(device)
        self.seed = int(seed)
        self.config = dict(config or {})
        self.num_actions = NUM_ACTIONS
        self.obs_dim = OBS_DIM
        self.time_scale = float(self.config.get("time_scale", 1.0))
        if self.time_scale <= 0:
            raise ValueError("time_scale must be positive")
        self.noise = float(self.config.get("noise", .1))
        self.distractor_rate = float(self.config.get("distractor_rate", .02))
        self.pulse_length = int(self.config.get("pulse_length", 2))
        self.osc_rate = float(self.config.get("oscillator_rate", .13))
        self.ou_theta = float(self.config.get("ou_theta", .08))
        self.horizon = int(self.config.get("horizon", 160))
        if self.horizon < 8:
            raise ValueError("horizon must be >=8")
        self.generator = torch.Generator(device=self.device).manual_seed(self.seed)
        self.t = 0
        self._initialized = False

    def _rand(self, *shape):
        return torch.rand(shape, generator=self.generator, device=self.device)

    def _randint(self, high, shape):
        return torch.randint(high, shape, generator=self.generator, device=self.device)

    def _init_streams(self):
        """Precomputed exogenous draws keep counterfactual pairing identical."""
        h, n = self.horizon, self.num_envs
        self.noise_stream = self._rand(h, n, OBS_DIM)
        self.distractor_stream = self._rand(h, n)
        self.distractor_value = self._rand(h, n)
        self.phase0 = self._rand(n) * 2 * torch.pi
        self.phase_jitter = ((self._rand(h, n) - .5) * .3).cumsum(0)
        self.ou_stream = (self._rand(h, n) - .5) * .6
        self.ou_state = torch.zeros(n, device=self.device)

    def _dynamics(self, obs):
        """Stochastic world-dynamics channels; rate follows time_scale."""
        s = self.time_scale
        phase = self.phase0 + self.t * self.osc_rate * s + self.phase_jitter[self.t] * s
        obs[:, 2] = .5 + .5 * torch.sin(phase)
        theta = self.ou_theta * s
        self.ou_state = self.ou_state + theta * (.5 - self.ou_state) + self.ou_stream[self.t] * s
        obs[:, 3] = self.ou_state.clamp(0, 1)
        pulse = self.distractor_stream[self.t] < self.distractor_rate
        obs[:, 4] = torch.where(pulse, .6 + .4 * self.distractor_value[self.t], obs[:, 4])

    def _base_observation(self):
        obs = self.noise_stream[self.t] * self.noise
        obs[:, 5:] += .05
        self._dynamics(obs)
        return obs

    def snapshot(self):
        state = {"t": self.t, "generator": self.generator.get_state()}
        for name, value in self.__dict__.items():
            if torch.is_tensor(value):
                state[name] = value.clone()
        return state

    def restore(self, state):
        self.t = state["t"]
        self.generator.set_state(state["generator"])
        for name, value in state.items():
            if name in ("t", "generator"):
                continue
            setattr(self, name, value.clone())

    def _current_observation(self):
        raise NotImplementedError


class IntervalEnv(BaseTemporalEnv):
    """T0-A interval production: act T* steps after a delay-encoding pulse."""

    def __init__(self, num_envs, device="cpu", seed=0, config=None):
        super().__init__(num_envs, device, seed, config)
        self.window = int(self.config.get("window", 2))
        self.delay_scale = float(self.config.get("delay_scale", 128.))
        self.grade = float(self.config.get("grade", 8.))
        self.never_reward = float(self.config.get("never_reward", -1.))
        self.step_cost = float(self.config.get("step_cost", .01))

    def _sample_delays(self):
        n = self.num_envs
        dist = self.config.get("delay_distribution", "grid")
        grid = list(self.config.get("delays", SEEN_DELAYS))
        lo = int(self.config.get("delay_min", min(grid)))
        hi = int(self.config.get("delay_max", max(grid)))
        if dist == "grid":
            values = torch.tensor(grid, device=self.device)
            return values[self._randint(len(grid), (n,))].long()
        if dist == "uniform":
            return self._randint(hi - lo + 1, (n,)).long() + lo
        if dist == "geometric":
            p = float(self.config.get("geometric_p", .08))
            draw = torch.distributions.Geometric(p).sample((n,)).to(self.device).long()
            return (lo + draw).clamp(min=lo, max=hi)
        if dist == "mixed":
            values = torch.tensor(grid, device=self.device)
            from_grid = values[self._randint(len(grid), (n,))].long()
            from_uniform = self._randint(hi - lo + 1, (n,)).long() + lo
            pick = self._rand(n) < .5
            return torch.where(pick, from_grid, from_uniform)
        raise ValueError(f"Unknown delay_distribution: {dist}")

    def reset(self, delay_override=None):
        n = self.num_envs
        self.t = 0
        self.delay = self._sample_delays()
        if delay_override is not None:
            supplied = torch.as_tensor(delay_override, device=self.device, dtype=torch.long)
            if supplied.numel() == 1:
                supplied = supplied.expand(n)
            if supplied.shape != (n,) or bool((supplied < 1).any()):
                raise ValueError("delay_override must be positive ints [num_envs]")
            self.delay = supplied.clone()
        self.target_step = torch.round(self.delay.float() / self.time_scale).long()
        self.target_step = self.target_step.clamp(self.pulse_length, self.horizon - 1)
        self._init_streams()
        self.acted = torch.zeros(n, dtype=torch.bool, device=self.device)
        self.act_step = torch.full((n,), -1, dtype=torch.long, device=self.device)
        self.timing_error = torch.zeros(n, dtype=torch.long, device=self.device)
        self._initialized = True
        self.observation = self._current_observation()
        return self.observation.clone()

    def _current_observation(self):
        obs = self._base_observation()
        pulsing = self.t < self.pulse_length
        obs[:, 0] = float(pulsing)
        obs[:, 1] = (self.delay.float() / self.delay_scale) if pulsing else 0.
        if self.config.get("blank_delay") and not pulsing:
            obs[:, :] = .5
        return obs.clamp_(0, 1)

    def oracle_actions(self):
        if not self._initialized or self.t >= self.horizon:
            raise RuntimeError("reset required")
        actions = torch.full((self.num_envs,), WAIT, dtype=torch.long, device=self.device)
        actions = torch.where(self.t >= self.target_step, torch.full_like(actions, ACT), actions)
        return actions

    def step(self, actions):
        if not self._initialized or self.t >= self.horizon:
            raise RuntimeError("reset required")
        actions = torch.as_tensor(actions, device=self.device, dtype=torch.long)
        if actions.shape != (self.num_envs,):
            raise ValueError("actions must be [num_envs]")
        acting = (actions == ACT) & ~self.acted
        error = self.t - self.target_step
        reward = torch.full((self.num_envs,), -self.step_cost, device=self.device)
        reward = torch.where(acting, (1. - error.abs().float() / self.grade).clamp(-1., 1.), reward)
        self.acted |= acting
        self.act_step = torch.where(acting, torch.full_like(self.act_step, self.t), self.act_step)
        self.timing_error = torch.where(acting, error, self.timing_error)
        self.t += 1
        done = self.acted | (self.t >= self.horizon)
        reward = torch.where((self.t >= self.horizon) & ~self.acted & ~acting,
                             torch.full_like(reward, self.never_reward), reward)
        info = {"delay": self.delay.clone(), "target_step": self.target_step.clone(),
                "acted": self.acted.clone(), "act_step": self.act_step.clone(),
                "timing_error": self.timing_error.clone(),
                "success": self.acted & (self.timing_error.abs() <= self.window),
                "early": self.acted & (self.timing_error < -self.window),
                "late": self.acted & (self.timing_error > self.window),
                "step": self.t}
        self.observation = self._current_observation() if self.t < self.horizon \
            else torch.zeros(self.num_envs, OBS_DIM, device=self.device)
        return self.observation.clone(), reward, done, info
