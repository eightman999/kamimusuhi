"""Closed-loop, numeric-only synthetic information acquisition environment."""
import copy
import torch

IGNORE, WAIT, ORIENT, OBSERVE, RECALL, INVOKE_LANGUAGE = range(6)
ACTION_NAMES = ("IGNORE", "WAIT", "ORIENT", "OBSERVE", "RECALL", "INVOKE_LANGUAGE")
SCENARIO_NAMES = ("language", "memory", "orient", "observe", "habituation", "irrelevant")
SENSOR_NAMES = ("motion", "motion_delta", "sound_energy", "light_level", "proximity", "touch",
                "temperature_delta", "task_relevance", "speech_activity", "acquired_value",
                "acquired_confidence", "resource_level", "language_cost", "language_reliability",
                "decision_ready", "language_latency")
RESPONSE_MODES = {"correct", "shuffled", "random", "inverted", "delayed", "missing",
                  "plausible_wrong", "contradictory", "uncertain"}
OOD_ALIASES = {"noise_increase": "noise", "sensor_dropout": "dropout", "sensor_inversion": "inversion",
               "long_delay": "longer_delay", "unknown_combinations": "combination", "resource_reduction": "resource",
               "sensor_failure": "failure", "sensor_fault": "failure", "sensor_permutation": "permutation",
               "constant_sensor": "constant", "stuck_at": "constant", "stuck-at": "constant",
               "delayed_sensor": "delayed", "correlated_noise": "correlated", "novel_scale": "scale",
               "novel_range": "scale", "sensor_channel_shuffle": "permutation"}
OOD_MODES = {None, "noise", "dropout", "inversion", "longer_delay", "combination", "resource", "failure",
             "partial_inversion", "permutation", "constant", "delayed", "correlated", "scale", "cue_mask",
             "selected_channel_mask", "prefix_removal", "suffix_removal", "time_shuffle", "latent_feature_removal"}


class ActiveInfoEnv:
    def __init__(self, num_envs, device="cpu", seed=0, config=None):
        self.num_envs = int(num_envs)
        self.device = torch.device(device)
        self.seed = int(seed)
        self.config = dict(config or {})
        self.episode_length = int(self.config.get("episode_length", 48))
        if "memory_delay" in self.config:
            self.episode_length = int(self.config["memory_delay"]) + 1
        self.ood = self.config.get("ood")
        if isinstance(self.ood, dict):
            self.ood = self.ood.get("kind")
        self.ood = OOD_ALIASES.get(self.ood, self.ood)
        if self.ood not in OOD_MODES:
            raise ValueError(f"Unknown OOD mode: {self.ood}")
        if self.ood == "longer_delay" and "memory_delay" not in self.config:
            self.episode_length *= 2
        if self.episode_length < 8:
            raise ValueError("episode_length must be >=8")
        if self.config.get("language_backend", "scripted") not in ("scripted", "external"):
            raise ValueError("language_backend must be scripted or external")
        self.response_mode = self.config.get("response_mode", "correct")
        if self.response_mode not in RESPONSE_MODES:
            raise ValueError(f"Unknown response_mode: {self.response_mode}")
        self.generator = torch.Generator(device=self.device).manual_seed(self.seed)
        self.t = 0
        self._initialized = False

    def _rand(self, *shape):
        return torch.rand(shape, generator=self.generator, device=self.device)

    def _sample(self, values):
        grid = torch.tensor(values, device=self.device)
        indices = torch.randint(len(values), (self.num_envs,), generator=self.generator, device=self.device)
        return grid[indices]

    def reset(self, latent_override=None):
        n, length = self.num_envs, self.episode_length
        self.t = 0
        self.scenario = torch.randint(6, (n,), generator=self.generator, device=self.device)
        selected = self.config.get("scenario")
        if selected is not None:
            aliases = {"ambiguous": "language", "language_ambiguity": "language", "delayed_cue": "memory",
                       "active_orient": "orient", "active_observe": "observe", "idle": "irrelevant"}
            selected = aliases.get(selected, selected)
            selected = SCENARIO_NAMES.index(selected) if isinstance(selected, str) else int(selected)
            if not 0 <= selected < 6:
                raise ValueError("scenario must be in 0..5")
            self.scenario.fill_(selected)
        self.latent = torch.randint(2, (n,), generator=self.generator, device=self.device)
        if latent_override is not None:
            supplied = torch.as_tensor(latent_override, device=self.device, dtype=torch.long)
            if supplied.shape != (n,) or not bool(((supplied == 0) | (supplied == 1)).all()):
                raise ValueError("latent_override must be binary [num_envs]")
            self.latent = supplied.clone()
        self.cost = torch.full((n,), float(self.config.get("language_cost", self.config.get("cost", .05))), device=self.device)
        self.reliability = torch.full((n,), float(self.config.get("language_reliability", self.config.get("reliability", 1.))), device=self.device)
        self.latency = torch.full((n,), int(self.config.get("language_latency", self.config.get("latency", 2))), dtype=torch.long, device=self.device)
        if self.config.get("mix_conditions", False):
            self.cost = self._sample(self.config.get("cost_grid", [0., .01, .02, .05, .1, .2, .4, .8, 1.2]))
            self.reliability = self._sample(self.config.get("reliability_grid", [1., .9, .75, .5]))
            self.latency = self._sample(self.config.get("latency_grid", [0, 1, 2, 4, 8, 16])).long()
        if bool(((self.cost < 0) | (self.reliability < 0) | (self.reliability > 1) | (self.latency < 0)).any()):
            raise ValueError("cost/latency must be nonnegative, reliability within [0,1]")
        # Precomputed, action-independent exogenous draws preserve counterfactual pairing.
        self.noise_stream = self._rand(length, n, 16)
        self.response_draw = self._rand(n)
        self.random_fact = (self._rand(n) > .5).long()
        self.permutation = torch.randperm(n, generator=self.generator, device=self.device)
        self.sensor_permutation = torch.randperm(9, generator=self.generator, device=self.device)
        self.time_permutation = torch.randperm(length, generator=self.generator, device=self.device)
        self.called = torch.zeros(n, dtype=torch.bool, device=self.device)
        self.pending = torch.zeros_like(self.called)
        self.delivered = torch.zeros_like(self.called)
        self.due = torch.full((n,), length + 1, dtype=torch.long, device=self.device)
        self.language_fact = torch.zeros(n, dtype=torch.long, device=self.device)
        self.language_confidence = torch.zeros(n, device=self.device)
        self.external_response_ready = torch.zeros(n, dtype=torch.bool, device=self.device)
        self.external_categories = torch.zeros(n, dtype=torch.long, device=self.device)
        self.external_confidences = torch.zeros(n, device=self.device)
        self.external_valid = torch.zeros(n, dtype=torch.bool, device=self.device)
        self.orient_remaining = torch.zeros(n, dtype=torch.long, device=self.device)
        self.observed = torch.zeros_like(self.called)
        self.recalled = torch.zeros_like(self.called)
        self.resource = torch.full((n,), .65 if self.ood != "resource" else .08, device=self.device)
        self.belief = torch.full((n,), -1, dtype=torch.long, device=self.device)
        self.belief_confidence = torch.zeros(n, device=self.device)
        self.observed_cue = torch.full((n,), -1, dtype=torch.long, device=self.device)
        self.observed_cue_value = torch.full((n,), .5, device=self.device)
        self.observed_cue_confidence = torch.zeros(n, device=self.device)
        self.habituation_ok = torch.ones(n, dtype=torch.bool, device=self.device)
        self._previous_raw = torch.zeros(n, 16, device=self.device)
        self._initialized = True
        self.observation = self._make_observation()
        self._update_belief()
        return self.observation.clone()

    def _habituation_masks(self):
        gap = max(1, min(int(self.config.get("habituation_gap", 3)), (self.episode_length - 3) // 5))
        repeat_count = min(int(self.config.get("repeat_count", 3)), max(1, (self.episode_length - 3) // gap - 1))
        first = self.scenario == 4 if self.t == 1 else torch.zeros_like(self.called)
        repeat = (self.scenario == 4) & (self.t > 1) & (self.t <= 1 + repeat_count * gap) & ((self.t - 1) % gap == 0)
        renewed = (self.scenario == 4) & (self.t == 1 + (repeat_count + 1) * gap)
        return first, repeat, renewed

    def _make_observation(self):
        noise_t = int(self.time_permutation[self.t]) if self.ood == "time_shuffle" else self.t
        noise = self.noise_stream[noise_t]
        obs = noise * .04
        obs[:, 3] = .5
        obs[:, 4] = .5 + (noise[:, 4] - .5) * .1
        obs[:, 6] = .5
        obs[:, 7] = torch.where(self.scenario == 5, .1, .9)
        obs[:, 0] += (self.scenario == 2) * .8
        obs[:, 1] += (self.scenario == 3) * .8
        speech = (self.scenario == 0) | (self.scenario == 5)
        obs[:, 2] += speech * .7
        obs[:, 8] += speech * .8
        obs[:, 9] = .5
        obs[:, 10] = 0.
        active_sensor = (self.scenario == 2) & (self.orient_remaining > 0)
        observed_sensor = (self.scenario == 3) & self.observed
        memory_cue = (self.scenario == 1) & ((self.t == 0) | self.recalled)
        visible = active_sensor | observed_sensor | memory_cue
        effective_fact = torch.where((self.scenario == 1) & self.recalled & (self.t > 0), self.observed_cue, self.latent)
        value = .25 + effective_fact.float() * .5
        sensory_noise = (noise[:, 9] - .5) * float(self.config.get("noise", .15))
        obs[:, 9] = torch.where(visible, value + sensory_noise, obs[:, 9])
        obs[:, 10] = torch.where(visible, .98, obs[:, 10])
        obs[:, 4] = torch.where(active_sensor, value + sensory_noise, obs[:, 4])
        obs[:, 5] = torch.where(memory_cue, value + sensory_noise, obs[:, 5])
        replay = (self.scenario == 1) & self.recalled & (self.t > 0)
        obs[:, 9] = torch.where(replay, self.observed_cue_value, obs[:, 9])
        obs[:, 5] = torch.where(replay, self.observed_cue_value, obs[:, 5])
        obs[:, 10] = torch.where(replay, self.observed_cue_confidence, obs[:, 10])
        usable_language = self.delivered & (self.scenario == 0) & (self.language_confidence > 0)
        obs[:, 9] = torch.where(usable_language, .25 + .5 * self.language_fact.float(), obs[:, 9])
        obs[:, 10] = torch.where(usable_language, self.language_confidence, obs[:, 10])
        first, repeat, renewed = self._habituation_masks()
        pulse = first | repeat | renewed
        obs[:, 0] = torch.where(pulse, .85, obs[:, 0])
        obs[:, 2] = torch.where(pulse, .6, obs[:, 2])
        obs[:, 5] = torch.where(renewed, float(self.config.get("novelty_strength", .95)), obs[:, 5])
        obs[:, 11] = self.resource
        if self.config.get("strict_memory", True):
            # Prevent a memoryless controller from writing a cue bit into its
            # future observations by choosing different resource expenditures.
            obs[:, 11] = torch.where(self.scenario == 1, .65, obs[:, 11])
        obs[:, 12] = self.cost / 1.2
        obs[:, 13] = self.reliability
        obs[:, 14] = float(self.t == self.episode_length - 1)
        obs[:, 15] = self.latency.float() / 16.
        raw = obs.clone()
        # Corrupt sensor/interface channels only; do not erase public decision protocol.
        if self.ood == "noise":
            obs[:, :11] += (noise[:, :11] - .5) * .7
        elif self.ood == "dropout":
            obs[:, :11] *= noise[:, :11] > .2
        elif self.ood in ("inversion", "partial_inversion"):
            count = 11 if self.ood == "inversion" else 4
            obs[:, :count] = 1 - obs[:, :count]
        elif self.ood == "permutation":
            obs[:, :9] = obs[:, self.sensor_permutation]
        elif self.ood in ("failure", "constant", "selected_channel_mask", "latent_feature_removal"):
            obs[:, 9] = .5
        elif self.ood == "delayed":
            obs[:, :11] = self._previous_raw[:, :11]
        elif self.ood == "correlated":
            obs[:, :11] += (noise[:, :1] - .5) * .6
        elif self.ood == "scale":
            obs[:, :11] *= 1.5
        elif self.ood == "combination":
            obs[:, 0] += .3
            obs[:, 2] += .25
        elif self.ood == "cue_mask" and self.t == 0:
            obs[:, 5] = 0.
            obs[:, 9] = .5
            obs[:, 10] = 0.
        elif self.ood == "prefix_removal" and self.t < self.episode_length // 4:
            obs[:, :11] = 0.
        elif self.ood == "suffix_removal" and self.t >= self.episode_length * 3 // 4:
            obs[:, :11] = 0.
        self._previous_raw = raw
        return obs.clamp_(0, 1)

    def _update_belief(self):
        # This teacher learns only from model-visible current/past measurements.
        confidence = self.observation[:, 10]
        value = (self.observation[:, 9] > .5).long()
        value = torch.where(confidence < .5, 1 - value, value)
        effective = torch.maximum(confidence, 1 - confidence)
        usable = (confidence > 0) & (effective > .5) & (effective > self.belief_confidence)
        self.belief = torch.where(usable, value, self.belief)
        self.belief_confidence = torch.where(usable, effective, self.belief_confidence)
        if self.t == 0:
            self.observed_cue = torch.where((self.scenario == 1) & usable, value, self.observed_cue)
            memory = (self.scenario == 1) & (confidence > 0)
            self.observed_cue_value = torch.where(memory, self.observation[:, 9], self.observed_cue_value)
            self.observed_cue_confidence = torch.where(memory, confidence, self.observed_cue_confidence)

    def voi(self):
        accuracy = torch.maximum(self.reliability, 1 - self.reliability)
        return 2 * (accuracy - .5) - self.cost - self.latency * float(self.config.get("wait_cost", .002))

    def oracle_actions(self):
        if not self._initialized or self.t >= self.episode_length:
            raise RuntimeError("reset required")
        actions = torch.full((self.num_envs,), IGNORE, dtype=torch.long, device=self.device)
        useful_call = (self.scenario == 0) & ~self.called & (self.voi() > 0) & (self.t + self.latency + 1 < self.episode_length)
        actions = torch.where(useful_call, INVOKE_LANGUAGE, actions)
        actions = torch.where(self.pending & (self.scenario == 0), WAIT, actions)
        actions = torch.where((self.scenario == 2) & (self.belief < 0), ORIENT, actions)
        actions = torch.where((self.scenario == 3) & (self.belief < 0), OBSERVE, actions)
        first, repeat, renewed = self._habituation_masks()
        actions = torch.where(first | renewed, ORIENT, actions)
        if self.t == self.episode_length - 1:
            # Unknown answer: deterministic Bayes guess A independent of hidden truth.
            answers = torch.where(self.belief == 1, RECALL, OBSERVE)
            actions = torch.where(self.scenario < 4, answers, IGNORE)
        return actions

    def _deliver(self, mask):
        if self.response_mode not in RESPONSE_MODES:
            raise ValueError(f"Unknown response mode: {self.response_mode}")
        fact = torch.where(self.response_draw < self.reliability, self.latent, 1 - self.latent)
        confidence = self.reliability.clone()
        if self.response_mode == "shuffled":
            fact = fact[self.permutation]
        elif self.response_mode == "random":
            fact = self.random_fact
        elif self.response_mode in ("inverted", "plausible_wrong"):
            fact = 1 - self.latent
        elif self.response_mode == "contradictory":
            fact = 1 - self.latent
            confidence.fill_(0.)
        elif self.response_mode == "uncertain":
            fact = self.random_fact
            confidence.fill_(.5)
        elif self.response_mode == "missing":
            confidence.fill_(0.)
        if self.config.get("language_backend", "scripted") == "external":
            # External mode never substitutes a latent-aware scripted answer.
            fact = torch.zeros_like(fact)
            confidence = torch.zeros_like(confidence)
        fact = torch.where(self.external_response_ready, self.external_categories, fact)
        confidence = torch.where(self.external_response_ready,
                                 torch.where(self.external_valid, self.external_confidences, 0.), confidence)
        confidence = torch.where(self.scenario == 0, confidence, 0.)
        self.language_fact = torch.where(mask, fact, self.language_fact)
        self.language_confidence = torch.where(mask, confidence, self.language_confidence)
        self.delivered |= mask
        self.pending &= ~mask

    def inject_language_response(self, indices, categories, confidences, valid):
        """Supply parsed external facts only for episodes which selected CALL.

        Injection before due time queues the result. For latency zero, the call
        transition has already exposed a missing result; injection updates the
        public current observation immediately, before the next Core forward.
        No raw text is consumed and categories are not checked against truth.
        Invalid schemas raise ValueError without mutating environment state.
        """
        if not self._initialized or self.t >= self.episode_length:
            raise RuntimeError("injection requires an active episode")
        index_raw = torch.as_tensor(indices, device=self.device)
        category_raw = torch.as_tensor(categories, device=self.device)
        confidence = torch.as_tensor(confidences, device=self.device, dtype=torch.float32)
        valid_raw = torch.as_tensor(valid, device=self.device)
        if index_raw.ndim != 1 or category_raw.shape != index_raw.shape or confidence.shape != index_raw.shape or valid_raw.shape != index_raw.shape:
            raise ValueError("indices/categories/confidences/valid must have matching 1D shapes")
        if index_raw.dtype not in (torch.int8, torch.int16, torch.int32, torch.int64, torch.uint8):
            raise ValueError("indices must be integers")
        indices = index_raw.long()
        if bool(((indices < 0) | (indices >= self.num_envs)).any()) or len(indices.unique()) != len(indices):
            raise ValueError("indices must be unique and within the environment batch")
        if category_raw.dtype not in (torch.int8, torch.int16, torch.int32, torch.int64, torch.uint8, torch.bool) or not bool(((category_raw == 0) | (category_raw == 1)).all()):
            raise ValueError("categories must contain strict binary integers")
        if valid_raw.dtype != torch.bool:
            raise ValueError("valid must contain booleans")
        if not bool((torch.isfinite(confidence) & (confidence >= 0) & (confidence <= 1)).all()):
            raise ValueError("confidences must be finite and within [0,1]")
        if not bool(self.called[indices].all()):
            raise ValueError("external information cannot be injected before CALL")
        self.external_response_ready[indices] = True
        self.external_categories[indices] = category_raw.long()
        self.external_confidences[indices] = confidence
        self.external_valid[indices] = valid_raw
        arrived = torch.zeros_like(self.called)
        arrived[indices] = self.due[indices] <= self.t
        if bool(arrived.any()):
            self._deliver(arrived)
            previous_raw = self._previous_raw.clone()
            self.observation = self._make_observation()
            self._previous_raw = previous_raw
            self._update_belief()
        return self.observation.clone()

    def step(self, actions):
        if not self._initialized or self.t >= self.episode_length:
            raise RuntimeError("reset required")
        actions = actions.to(device=self.device, dtype=torch.long)
        if actions.shape != (self.num_envs,):
            raise ValueError("actions must have shape [num_envs]")
        decision = torch.full_like(self.called, self.t == self.episode_length - 1)
        oracle = self.oracle_actions()
        first, repeat, renewed = self._habituation_masks()
        pulse = first | repeat | renewed
        required_action = torch.where(first | renewed, ORIENT, IGNORE)
        self.habituation_ok &= ~pulse | (actions == required_action)
        truth = torch.where(self.latent == 0, OBSERVE, RECALL)
        truth = torch.where(self.scenario >= 4, IGNORE, truth)
        success = decision & (actions == truth)
        success &= (self.scenario != 4) | self.habituation_ok
        acquisition_allowed = ~decision
        accepted = acquisition_allowed & (actions == INVOKE_LANGUAGE) & ~self.called
        physical_language_cost = accepted * self.cost
        language_cost = physical_language_cost * (0. if self.config.get("zero_call_cost", False) else 1.)
        orient = acquisition_allowed & (actions == ORIENT)
        observe = acquisition_allowed & (actions == OBSERVE)
        recall = acquisition_allowed & (actions == RECALL)
        resource_cost = orient * float(self.config.get("orient_cost", .01)) + observe * float(self.config.get("observe_cost", .02)) + recall * float(self.config.get("recall_cost", .01))
        unit_wait_cost = float(self.config.get("wait_cost", .002))
        latency_cost = acquisition_allowed * self.pending * unit_wait_cost
        wait_action_cost = acquisition_allowed * ~self.pending * (actions == WAIT) * unit_wait_cost
        reward = -language_cost - resource_cost - latency_cost - wait_action_cost
        reward += torch.where(decision, torch.where(success, 1., -1.), 0.)
        reward += torch.where(pulse, torch.where(actions == required_action, .05, -.02), 0.)
        call_required = (self.scenario == 0) & (self.voi() > 0)
        self.called |= accepted
        self.pending |= accepted
        extra = 4 if self.response_mode == "delayed" else 0
        self.due = torch.where(accepted, self.t + 1 + self.latency + extra, self.due)
        self.orient_remaining = torch.clamp(self.orient_remaining - 1, min=0)
        duration = max(2, min(4, int(self.config.get("orient_duration", 3))))
        self.orient_remaining = torch.where(orient, duration, self.orient_remaining)
        self.observed |= observe & (self.scenario == 3)
        self.recalled = recall & (self.scenario == 1) & (self.observed_cue >= 0)
        if self.config.get("strict_memory", True):
            self.recalled &= False
        self.resource = (self.resource - resource_cost - physical_language_cost * .1 + (actions == WAIT) * .03).clamp(0, 1)
        self.t += 1
        delivered_now = self.pending & (self.due <= self.t)
        self._deliver(delivered_now)
        done = torch.full_like(self.called, self.t == self.episode_length)
        if self.t < self.episode_length:
            self.observation = self._make_observation()
            self._update_belief()
        info = dict(scenario=self.scenario.clone(), target=truth, true_target=truth,
                    decision_mask=decision, success=success, call_required=call_required,
                    required_llm=call_required, call_accepted=accepted, language_cost=language_cost,
                    resource_cost=resource_cost, physical_language_cost=physical_language_cost, latency_cost=latency_cost, wait_action_cost=wait_action_cost, acquisition=orient | observe | recall | accepted,
                    language_delivered=delivered_now, language_result=self.language_fact.clone(),
                    language_confidence=self.language_confidence.clone(), first_stimulus_mask=first,
                    habituation_mask=repeat, renewed_stimulus_mask=renewed, novelty_mask=first | renewed,
                    retention_mask=decision & (self.scenario == 1), oracle_action=oracle,
                    final_success=success, required_language=call_required, resource=self.resource.clone())
        return self.observation.clone(), reward, done, info

    def snapshot(self):
        result = {key: value.clone() if isinstance(value, torch.Tensor) else copy.deepcopy(value)
                  for key, value in self.__dict__.items() if key != "generator"}
        result["rng_state"] = self.generator.get_state().clone()
        return result

    state_dict = snapshot

    def restore(self, state):
        for key, value in state.items():
            if key != "rng_state":
                setattr(self, key, value.clone() if isinstance(value, torch.Tensor) else copy.deepcopy(value))
        self.generator = torch.Generator(device=self.device)
        self.generator.set_state(state["rng_state"])
        return self

    load_state_dict = restore

    def clone(self):
        return ActiveInfoEnv(self.num_envs, self.device, self.seed, self.config).restore(self.snapshot())
