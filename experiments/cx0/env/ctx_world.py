"""CX0 shared world kernel + CTX-1..CTX-5 task wrappers + scripted oracles.

One ``CtxWorld`` kernel supports every CTX task: a ring of sites with
per-episode assigned functions, five homeostatic internal variables,
action-coupled vs exogenous sensory dims, discoverable site events, a
delay-encoding cue channel, and a fixed 11-action space. Task wrappers
parameterize the event/crisis schedule, success rule, and oracle.

Sensory layout (16 dims, all the integration layer may see raw):
  dims 0-3   SELF dims: pos_sin, pos_cos, move_vel (decaying last move),
             rest_flag      — action-coupled (S0 domain)
  dims 4-7   WORLD dims: exogenous AR(1) process + impulse events
  dims 8-11  EVENT dims: [present, etype_id/N_ETYPES, strength, phantom_flag?]
             perceived only while standing on the announcing site
             (ambient cue events fill these dims regardless of position)
  dim 12     CUE channel: pulse intensity (encodes delay /96 when active)
  dim 13     GO flag: response-window indicator (used by some tasks only)
  dims 14-15 read noise

Internal vars (H0 domain, NOT in sensory): [energy, fatigue, temperature,
novelty, certainty]. They reach the integration layer only through the H0
organ signal — keeping the organ boundary real.

Actions: 0 NOOP, 1 FWD, 2 BACK, 3 INTERACT, 4 REST, 5 STORE, 6 RECALL,
7..10 RESP0..3.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np

N_LOCS = 8
OBS_DIM = 16
N_INTERNAL = 5
N_ACTIONS = 11
N_ETYPES = 6  # 0 none, 1 FOOD, 2 SHELTER, 3 REST, 4 HAZARD, 5 CUE
EP_LEN = 80
# v4 candidate fix (runs_v3, 10 seeds): a 0.02 cost on identical-payload
# re-recalls was TESTED AND REJECTED — it cut c2/ctx4 dithering (+0.098)
# but regressed c2/ctx3 (−0.114) and ctx2 broadly; net c2 −0.011.
# Kept at 0.0 so the mechanism stays documented but inert.
RECALL_REDUNDANT_COST = 0.0

NOOP, FWD, BACK, INTERACT, REST, STORE, RECALL = range(7)
RESP0 = 7

SELF_DIMS = slice(0, 4)
WORLD_DIMS = slice(4, 8)
EVENT_DIMS = slice(8, 12)
CUE_DIM = 12
GO_DIM = 13

ENERGY, FATIGUE, TEMP, NOVELTY, CERTAINTY = range(N_INTERNAL)
NEED_TO_ETYPE = {ENERGY: 1, TEMP: 2, FATIGUE: 3}  # crisis var -> serving etype


@dataclass
class EventSpec:
    etype: int
    site: int | None          # None => ambient (CTX-3 cue)
    t_start: int
    t_dur: int
    strength: float = 1.0     # extra info channel (CTX-3 response index /3)


@dataclass
class TaskSpec:
    name: str
    site_etypes: tuple[int, ...]        # potent functional sites placed per episode
    n_decoys: int = 0                   # HAZARD sites
    announce_window: tuple[int, int] = (8, 24)
    crisis_delay_range: tuple[int, int] = (44, 60)
    cue_at: int = 2
    go_window: tuple[int, int] | None = None
    probe_times: tuple[int, ...] = ()   # CTX-5
    phantom_prob: float = 0.0           # CTX-2 self-caused event noise
    resp_delay_range: tuple[int, int] = (24, 72)  # CTX-3


TASKS: dict[str, TaskSpec] = {
    # CTX-1 — H0+R0: a need crisis fires; the serving site was discoverable
    # only during the announce window. Which need arrives is internal-only.
    "ctx1": TaskSpec(
        name="ctx1", site_etypes=(1, 2, 3, 1, 2, 3), n_decoys=1,
        crisis_delay_range=(36, 50),
    ),
    # CTX-2 — S0+R0: phantom self-caused event pulses vs true site events;
    # only the world-caused announcement marks the real KEY site.
    "ctx2": TaskSpec(
        name="ctx2", site_etypes=(1,), n_decoys=3,
        announce_window=(6, 30), go_window=(48, 66), phantom_prob=0.35,
    ),
    # CTX-3 — T0+R0: ambient CUE event encodes response k (strength) and
    # delay D (cue-channel intensity). No go flag: internal timing required.
    "ctx3": TaskSpec(
        name="ctx3", site_etypes=(), announce_window=(2, 6),
        resp_delay_range=(20, 60),
    ),
    # CTX-4 — H0+T0+R0: warning cue at cue_at encodes crisis delay; at onset
    # the lowest drifting var crises; must be at the matching site and act.
    "ctx4": TaskSpec(
        name="ctx4", site_etypes=(1, 2, 3, 1, 2, 3), n_decoys=1,
        announce_window=(6, 20), cue_at=4, crisis_delay_range=(30, 52),
    ),
    # CTX-5 — conflict: decision probes where drives disagree.
    "ctx5": TaskSpec(
        name="ctx5", site_etypes=(1, 2, 3, 1, 2, 3), n_decoys=1,
        announce_window=(6, 20), probe_times=(30, 40, 50, 60, 70),
    ),
}


@dataclass
class StepInfo:
    t: int
    obs: np.ndarray
    internal: np.ndarray
    event_etype: int          # currently perceived etype (0=none)
    event_site: int | None    # site the perceived event belongs to (or pos for ambient cue)
    event_strength: float
    phantom: bool             # perceived event dims are self-caused this step
    dead: bool
    reward: float
    success: bool             # task success latch
    go: bool


class CtxWorld:
    """Shared kernel. `task` selects the schedule; `spec` may override TASKS."""

    def __init__(self, task: str, seed: int = 0, spec: TaskSpec | None = None):
        self.task = task
        self.spec = spec or TASKS[task]
        self.rng = np.random.default_rng(seed)
        self.seed = seed

        # --- per-episode layout -------------------------------------
        self.sites = np.zeros(N_LOCS, dtype=np.int64)  # etype per site (0=none)
        locs = self.rng.permutation(N_LOCS)
        for i, et in enumerate(self.spec.site_etypes):
            self.sites[locs[i]] = et
        for j in range(self.spec.n_decoys):
            self.sites[locs[len(self.spec.site_etypes) + j]] = 4  # HAZARD

        self.pos = int(self.rng.integers(N_LOCS))
        self.t = 0
        self.internal = np.array([0.8, 0.1, 0.55, 0.3, 0.9], dtype=np.float64)
        self.internal += self.rng.normal(0, 0.03, N_INTERNAL)
        self.internal = np.clip(self.internal, 0.05, 0.98)

        self.world_proc = self.rng.normal(0, 0.5, 4)
        self.move_vel = 0.0
        self.rest_flag = 0.0
        self.dead = False
        self.success = False
        self.last_recall = (np.zeros(8, dtype=np.float32), -1.0, -1)  # pl, score, slot

        # --- task-specific schedule ----------------------------------
        s = self.spec
        self.cue_delay = 0
        self.cue_resp = 0
        self.crisis_t = -1
        self.crisis_need = -1
        self.probe_idx = 0
        self._need_drift = np.zeros(N_INTERNAL)
        self._answer_failed = False   # CTX-2: wrong one-shot INTERACT used
        self.crisis_onset = -1        # step the crisis need was picked
        self._phantom_ttl = 0         # CTX-2: phantom pulses persist 2 steps
        self._phantom_et = 0
        # perception is rolled once per step (t-keyed cache); every
        # consumer — step, obs, recall query, cause labels, oracle —
        # must see the SAME event and burn the phantom TTL once per
        # step, not once per call
        self._evt_cache_t = -1
        self._evt_cache = (0, 0.0, False)

        if task == "ctx3":
            self.cue_delay = int(self.rng.integers(*s.resp_delay_range))
            self.cue_resp = int(self.rng.integers(0, 4))
        if task in ("ctx1", "ctx4"):
            rel = s.crisis_delay_range
            self.crisis_t = (int(self.rng.integers(*rel))
                             if task == "ctx1"
                             else s.cue_at + int(self.rng.integers(*rel)))
            # per-episode drift profile decides which var crises (u0-style)
            self._need_drift = self.rng.uniform(0.0, 0.004, N_INTERNAL)
            self._need_drift[[NOVELTY]] = 0.0

    # ------------------------------------------------------------------
    # observation
    # ------------------------------------------------------------------
    def _perceived_event(self) -> tuple[int, float, bool]:
        """(etype, strength, phantom) currently visible on event dims.

        Rolled once per env step and cached — all per-step consumers see
        the same event: the obs the agent acted on is what STORE writes
        and what the oracle/cause labels read. Phantom TTL decrements
        once per step, giving real 2-step persistence."""
        if self._evt_cache_t == self.t:
            return self._evt_cache
        s = self.spec
        t = self.t
        et, strength, phantom = 0, 0.0, False
        if self.task == "ctx3" and s.announce_window[0] <= t < s.announce_window[1]:
            et, strength = 5, self.cue_resp / 3.0       # ambient cue
        elif s.announce_window[0] <= t < s.announce_window[1]:
            e = int(self.sites[self.pos])
            if e != 0:
                et, strength = e, 1.0
        # CTX-2 phantom: self-caused pulses (action-correlated), 2-step
        # persistence so attribution (S0, one step behind) can gate STORE.
        # Dense during announce so naive store-everything fails; S0 gating
        # is what makes the task solvable reliably.
        if et == 0 and s.phantom_prob > 0:
            if self._phantom_ttl > 0:
                self._phantom_ttl -= 1
                et, strength, phantom = self._phantom_et, 0.6, True
            else:
                in_announce = s.announce_window[0] <= t < s.announce_window[1]
                rate = s.phantom_prob * (2.0 if in_announce else 0.15)
                if self.rng.random() < rate * (0.4 + 0.6 * abs(self.move_vel)):
                    self._phantom_et = int(self.rng.integers(1, 5))
                    self._phantom_ttl = 1
                    et, strength, phantom = (self._phantom_et,
                                             float(self.rng.uniform(0.4, 1.0)),
                                             True)
        self._evt_cache_t = t
        self._evt_cache = (et, strength, phantom)
        return self._evt_cache

    def _cue_level(self) -> float:
        s = self.spec
        if self.task == "ctx3" and self.t == s.cue_at:
            return self.cue_delay / 80.0
        if self.task == "ctx4" and self.t == s.cue_at:
            return (self.crisis_t - s.cue_at) / 80.0
        return 0.0

    def _go(self) -> bool:
        s = self.spec
        if s.go_window and s.go_window[0] <= self.t <= s.go_window[1]:
            return True
        if self.task == "ctx5" and self.t in s.probe_times:
            return True
        return False

    def _obs(self) -> np.ndarray:
        obs = np.zeros(OBS_DIM, dtype=np.float64)
        obs[0] = np.sin(2 * np.pi * self.pos / N_LOCS)
        obs[1] = np.cos(2 * np.pi * self.pos / N_LOCS)
        obs[2] = self.move_vel
        obs[3] = self.rest_flag
        obs[WORLD_DIMS] = self.world_proc
        et, strength, _phantom = self._perceived_event()
        obs[8] = 1.0 if et else 0.0
        obs[9] = et / N_ETYPES
        obs[10] = strength
        obs[CUE_DIM] = self._cue_level()
        obs[GO_DIM] = float(self._go())
        obs[14:16] = self.rng.normal(0, 0.05, 2)
        return obs

    def cause_labels(self) -> np.ndarray:
        """Pretraining-only: per-dim cause (0=self, 1=world, 2=noise)."""
        labels = np.full(OBS_DIM, 1, dtype=np.int64)
        labels[SELF_DIMS] = 0
        labels[14:16] = 2
        _et, _st, phantom = self._perceived_event()
        labels[EVENT_DIMS] = 0 if phantom else 1
        return labels

    def self_dims_mask(self) -> np.ndarray:
        m = np.zeros(OBS_DIM, dtype=np.float32)
        m[SELF_DIMS] = 1.0
        return m

    # ------------------------------------------------------------------
    # dynamics
    # ------------------------------------------------------------------
    def step(self, action: int, memory=None) -> StepInfo:
        p = self.spec
        et, strength, phantom = self._perceived_event()
        reward = 0.0
        prev_pos = self.pos

        # movement / internal costs
        if action == FWD:
            self.pos = (self.pos + 1) % N_LOCS
            self.move_vel = 1.0
            self.internal[ENERGY] -= 0.005
            self.internal[FATIGUE] += 0.015
            self.internal[NOVELTY] += 0.02
        elif action == BACK:
            self.pos = (self.pos - 1) % N_LOCS
            self.move_vel = -1.0
            self.internal[ENERGY] -= 0.005
            self.internal[FATIGUE] += 0.015
            self.internal[NOVELTY] += 0.02
        elif action == REST:
            self.internal[FATIGUE] -= 0.15
            self.internal[ENERGY] -= 0.004
            self.internal[TEMP] += 0.03 * (0.55 - self.internal[TEMP])
            self.rest_flag = 1.0
        elif action == INTERACT:
            reward += self._interact()
            # CTX-2 one-shot answer: wrong-site INTERACT inside the go
            # window burns the attempt — memory becomes load-bearing
            if self.task == "ctx2" and self._go() and \
                    int(self.sites[self.pos]) != 1:
                self._answer_failed = True
                reward -= 1.0
            # CTX-1/CTX-4: blind tour-and-interact is not free during a
            # crisis — wrong-site attempts cost, so recall actually pays
            if self.task in ("ctx1", "ctx4") and self.crisis_need >= 0 and \
                    int(self.sites[self.pos]) != NEED_TO_ETYPE[self.crisis_need]:
                reward -= 0.4
        elif action == STORE and memory is not None:
            # only a perceived event is storable — writing on et=0 used
            # to burn a slot under a zero key no query could match
            if et:
                payload = self._event_payload(et, strength)
                key = np.zeros(6, dtype=np.float32)
                key[et] = 1.0
                memory.store(payload, key)
        elif action == RECALL and memory is not None:
            prev_pl, prev_sc, _s = self.last_recall
            self.last_recall = memory.recall(self._recall_query())
            pl, sc, _s = self.last_recall
            if sc > 0.5 and prev_sc > 0.5 and np.array_equal(pl, prev_pl):
                reward -= RECALL_REDUNDANT_COST
        elif action >= RESP0:
            reward += self._respond(action - RESP0)

        # navigation shaping (PPO signal; disclosed): reward moving toward
        # the site named by the last retrieval, small bonus on arrival —
        # active only when a goal exists (crisis or go window)
        pl_r, sc_r, _sr = self.last_recall
        shaping_on = ((self.task in ("ctx1", "ctx4") and self.crisis_need >= 0)
                      or (self.task == "ctx2" and self._go()))
        if sc_r > 0.5 and shaping_on and action in (FWD, BACK):
            tgt = _payload_site(pl_r)
            d_b = _ring_dist(prev_pos, tgt)
            d_a = _ring_dist(self.pos, tgt)
            reward += 0.05 * (d_b - d_a)
            if d_a == 0:
                reward += 0.15

        # intrinsic drift + task drift (decides crisis need)
        self.internal[ENERGY] -= 0.0022 + self._need_drift[ENERGY]
        self.internal[FATIGUE] += 0.002 + self._need_drift[FATIGUE]
        self.internal[TEMP] += 0.02 * (self._temp_set - self.internal[TEMP]) - self._need_drift[TEMP]
        self.internal[NOVELTY] -= 0.005
        self.internal[CERTAINTY] -= 0.0015
        self.internal[FATIGUE] = np.clip(self.internal[FATIGUE], 0.0, 1.2)
        self.internal = np.clip(self.internal, -0.05, 1.2)

        # world process: AR(1) + impulses
        self.world_proc = 0.95 * self.world_proc + self.rng.normal(0, 0.08, 4)
        if self.rng.random() < 0.04:
            self.world_proc += self.rng.normal(0, 0.6, 4)

        self.move_vel *= 0.5
        self.rest_flag *= 0.5
        self.t += 1

        # crisis dynamics
        if self.crisis_t >= 0 and self.t >= self.crisis_t and self.crisis_need < 0:
            self.crisis_need = self._pick_crisis_need()
            self.crisis_onset = self.t
        if self.crisis_need >= 0:
            v = self.crisis_need
            if v == FATIGUE:
                self.internal[FATIGUE] = min(1.2, self.internal[FATIGUE] + 0.035)
            else:
                self.internal[v] -= 0.03
            self.internal[CERTAINTY] -= 0.008  # unresolved crisis erodes certainty

        # death
        if self.internal[ENERGY] <= 0.0 or self.internal[CERTAINTY] <= 0.05:
            self.dead = True
            reward -= 1.0

        success = self._check_success(action)
        if success:
            self.success = True
            reward += 1.0
            self.crisis_need = -1   # crisis resolved
            self.crisis_t = -1      # single crisis per episode
        # episode-end miss penalty: silence is not free
        if self.t >= EP_LEN and self.task in ("ctx3", "ctx2") \
                and not self.success and not self.dead:
            reward -= 0.3
        reward += 0.02 * float(np.mean(self.internal[[ENERGY, TEMP]]) > 0.4)

        return StepInfo(
            t=self.t, obs=self._obs(), internal=self.internal.copy(),
            event_etype=et, event_site=self.pos if et and not self._is_ambient(et) else None,
            event_strength=strength, phantom=phantom, dead=self.dead,
            reward=reward, success=success, go=self._go(),
        )

    # ------------------------------------------------------------------
    # task mechanics
    # ------------------------------------------------------------------
    @property
    def _temp_set(self) -> float:
        if not hasattr(self, "_temp_setpoint"):
            self._temp_setpoint = float(self.rng.choice([0.3, 0.55, 0.8]))
        return self._temp_setpoint

    def _is_ambient(self, et: int) -> bool:
        return et == 5

    def _event_payload(self, et: int, strength: float) -> np.ndarray:
        # site encoded as (sin, cos) — SAME representation as sensory
        # self dims, so recall→navigate is a direct representation match
        pl = np.zeros(8, dtype=np.float32)
        if et:
            pl[et - 1] = 1.0                      # etype onehot (5)
        pl[5] = np.sin(2 * np.pi * self.pos / N_LOCS)
        pl[6] = np.cos(2 * np.pi * self.pos / N_LOCS)
        pl[7] = strength
        return pl

    def _recall_query(self) -> np.ndarray:
        """Need-conditioned retrieval key: the active crisis's etype,
        else the currently perceived etype; CTX-3 queries the CUE memory."""
        q = np.zeros(6, dtype=np.float32)
        if self.task == "ctx3" and self.t >= self.spec.announce_window[1]:
            q[5] = 1.0
            return q
        if self.task == "ctx2" and self._go():
            q[1] = 1.0
            return q
        if self.crisis_need >= 0 and self.crisis_need in NEED_TO_ETYPE:
            q[NEED_TO_ETYPE[self.crisis_need]] = 1.0
        else:
            et, _st, _ph = self._perceived_event()
            if et:
                q[et] = 1.0
        return q

    def _pick_crisis_need(self) -> int:
        if self.task == "ctx1":
            # need that drifted lowest relative to safe range
            dev = np.array([1.0 - self.internal[ENERGY],
                            self.internal[FATIGUE],
                            abs(self.internal[TEMP] - 0.55)])
            return int(np.argmax(dev) if dev.argmax() == 0 else
                       [ENERGY, FATIGUE, TEMP][int(np.argmax(dev))])
        if self.task == "ctx4":
            dev = np.array([self._need_drift[ENERGY], self._need_drift[FATIGUE],
                            self._need_drift[TEMP]])
            return int([ENERGY, FATIGUE, TEMP][int(np.argmax(dev))])
        return ENERGY

    def _interact(self) -> float:
        et = int(self.sites[self.pos])
        if et == 1:
            self.internal[ENERGY] = min(1.0, self.internal[ENERGY] + 0.5)
            self.internal[CERTAINTY] += 0.05
            return 0.1
        if et == 2:
            self.internal[TEMP] += 0.4 * (0.55 - self.internal[TEMP])
            self.internal[CERTAINTY] += 0.05
            return 0.1
        if et == 3:
            self.internal[FATIGUE] = max(0.0, self.internal[FATIGUE] - 0.5)
            self.internal[CERTAINTY] += 0.05
            return 0.1
        if et == 4:
            self.internal[ENERGY] -= 0.15
            self.internal[CERTAINTY] -= 0.15
            return -0.3
        return -0.02

    def _respond(self, k: int) -> float:
        if self.task == "ctx3":
            in_window = self.cue_delay <= self.t <= self.cue_delay + 5
            if in_window and k == self.cue_resp:
                self.success = True
                return 1.0
            return -0.05          # mild: silence is worse (miss penalty at end)
        if self.task == "ctx5":
            # probe-step reward shaping: correct context response pays off
            if self.t in self.spec.probe_times:
                return 0.3 if k == int(np.argmax(self.probe_options())) else -0.1
            return 0.0   # success still measured by probe agreement
        return -0.05

    def _memory_proof(self, et: int) -> bool:
        """C-G3 gate: the agent must have *retrieved* the needed etype from
        slot memory — the last recall scored and its payload carries this
        etype. Hidden-state-only policies cannot pass; duplicate-site
        instance ambiguity is tolerated (position need not match)."""
        pl, score, _s = self.last_recall
        return score > 0.5 and bool(pl[et - 1] > 0.5)

    def _check_success(self, action: int) -> bool:
        if self.task == "ctx1":
            return (self.crisis_need >= 0 and action == INTERACT
                    and int(self.sites[self.pos]) == NEED_TO_ETYPE[self.crisis_need]
                    and self._memory_proof(NEED_TO_ETYPE[self.crisis_need]))
        if self.task == "ctx2":
            if self._answer_failed:
                return False
            return (self._go() and action == INTERACT
                    and int(self.sites[self.pos]) == 1
                    and self._memory_proof(1))
        if self.task == "ctx3":
            return action >= RESP0 and (action - RESP0) == self.cue_resp \
                and self.cue_delay <= self.t <= self.cue_delay + 5
        if self.task == "ctx4":
            return (self.crisis_need >= 0 and action == INTERACT
                    and int(self.sites[self.pos]) == NEED_TO_ETYPE[self.crisis_need]
                    and self._memory_proof(NEED_TO_ETYPE[self.crisis_need]))
        if self.task == "ctx5":
            return False   # success measured as probe agreement, set by eval
        return False

    # ------------------------------------------------------------------
    # probes (CTX-5) and context labels (for hidden-state decoding, C-G6)
    # ------------------------------------------------------------------
    def probe_options(self) -> np.ndarray:
        """CTX-5: value of each candidate response at a probe step.
        RESP0=seek food, RESP1=rest, RESP2=retreat, RESP3=explore."""
        food_sites = np.flatnonzero(self.sites == 1)
        rest_sites = np.flatnonzero(self.sites == 3)
        hazard_sites = np.flatnonzero(self.sites == 4)
        d_food = _ring_dist(self.pos, food_sites) if len(food_sites) else N_LOCS
        d_rest = _ring_dist(self.pos, rest_sites) if len(rest_sites) else N_LOCS
        d_hz = _ring_dist(self.pos, hazard_sites) if len(hazard_sites) else N_LOCS
        hunger = 1.0 - self.internal[ENERGY]
        tired = self.internal[FATIGUE]
        danger = max(0.0, (3 - d_hz) / 3.0)
        bored = 1.0 - self.internal[NOVELTY]
        return np.array([
            hunger * (1.0 - d_food / N_LOCS),          # seek food
            tired * (1.0 - d_rest / N_LOCS),           # rest
            danger * 1.2,                              # retreat
            0.25 * bored,                              # explore
        ])

    def context_label(self) -> int:
        """Task context variable for C-G6 hidden-state probing."""
        if self.task == "ctx1":
            return int(self.crisis_need) if self.crisis_need >= 0 else -1
        if self.task == "ctx3":
            return int(self.cue_resp)
        if self.task == "ctx4":
            return int(self.crisis_need) if self.crisis_need >= 0 else self._pick_crisis_need()
        if self.task == "ctx5":
            return int(np.argmax(self.probe_options()))
        return int(self.sites[self.pos])

    def danger_levels(self) -> np.ndarray:
        """Pretraining label for H0Organ: per-need danger in [0,1].
        Order: [energy, fatigue, temperature]."""
        e, f, t = (self.internal[ENERGY], self.internal[FATIGUE],
                   self.internal[TEMP])
        return np.array([
            np.clip((0.35 - e) * 2.5, 0.0, 1.0),
            np.clip((f - 0.75) * 4.0, 0.0, 1.0),
            np.clip((abs(t - 0.55) - 0.18) * 4.0, 0.0, 1.0),
        ])

    # ------------------------------------------------------------------
    # state snapshot (interventions)
    # ------------------------------------------------------------------
    def get_state(self) -> dict:
        return dict(pos=self.pos, t=self.t, internal=self.internal.copy(),
                    world_proc=self.world_proc.copy(), move_vel=self.move_vel,
                    rest_flag=self.rest_flag, dead=self.dead,
                    success=self.success, crisis_need=self.crisis_need,
                    crisis_t=self.crisis_t, cue_delay=self.cue_delay,
                    cue_resp=self.cue_resp, last_recall=(self.last_recall[0].copy(),
                                                         self.last_recall[1],
                                                         self.last_recall[2]),
                    temp_setpoint=getattr(self, "_temp_setpoint", None),
                    need_drift=self._need_drift.copy(),
                    answer_failed=self._answer_failed,
                    crisis_onset=self.crisis_onset, probe_idx=self.probe_idx,
                    phantom_ttl=self._phantom_ttl, phantom_et=self._phantom_et,
                    evt_cache_t=self._evt_cache_t,
                    evt_cache=tuple(self._evt_cache),
                    rng=self.rng.bit_generator.state)

    def set_state(self, st: dict) -> None:
        self.pos = st["pos"]; self.t = st["t"]
        self.internal = st["internal"].copy()
        self.world_proc = st["world_proc"].copy()
        self.move_vel = st["move_vel"]; self.rest_flag = st["rest_flag"]
        self.dead = st["dead"]; self.success = st["success"]
        self.crisis_need = st["crisis_need"]; self.crisis_t = st["crisis_t"]
        self.cue_delay = st["cue_delay"]; self.cue_resp = st["cue_resp"]
        self.last_recall = (st["last_recall"][0].copy(),
                            st["last_recall"][1], st["last_recall"][2])
        if st["temp_setpoint"] is not None:
            self._temp_setpoint = st["temp_setpoint"]
        self._need_drift = st["need_drift"].copy()
        self._answer_failed = st["answer_failed"]
        self.crisis_onset = st["crisis_onset"]
        self.probe_idx = st["probe_idx"]
        self._phantom_ttl = st["phantom_ttl"]
        self._phantom_et = st["phantom_et"]
        self._evt_cache_t = st["evt_cache_t"]
        self._evt_cache = tuple(st["evt_cache"])
        self.rng.bit_generator.state = st["rng"]


def _ring_dist(pos: int, sites: np.ndarray) -> float:
    d = np.abs(sites - pos) % N_LOCS
    return float(np.min(np.minimum(d, N_LOCS - d)))


# ----------------------------------------------------------------------
# Scripted oracles (BC teachers + heuristic control)
# ----------------------------------------------------------------------
class Oracle:
    """Privileged teacher: reads true world state. Deterministic."""

    def __init__(self, task: str):
        self.task = task
        self._stored = set()
        self._plan_site: int | None = None
        self._moved_this_probe = 0
        self._recalled_onset = -1

    def reset(self) -> None:
        self._stored = set()
        self._plan_site = None
        self._recalled_onset = -1

    def act(self, w: CtxWorld, memory) -> int:
        m = getattr(self, f"_act_{w.task}")
        return m(w, memory)

    # -- helpers ---------------------------------------------------------
    def _goto(self, w: CtxWorld, site: int) -> int:
        d = (site - w.pos) % N_LOCS
        return FWD if d <= N_LOCS // 2 else BACK

    def _store_here(self, w: CtxWorld, et: int) -> int | None:
        # never store HAZARD (et=4): 3 slots are for the 3 serving functions
        if et and et != 4 and et not in self._stored and self._perceives(w) == et:
            self._stored.add(et)
            return STORE
        return None

    @staticmethod
    def _perceives(w: CtxWorld) -> int:
        et, _st, ph = w._perceived_event()
        return 0 if ph else et

    # -- CTX-1: discover sites early; on crisis recall & serve -----------
    def _act_ctx1(self, w: CtxWorld, memory) -> int:
        s = w.spec
        if w.crisis_need < 0:
            if s.announce_window[0] <= w.t < s.announce_window[1]:
                et = self._perceives(w)
                st = self._store_here(w, et)
                if st is not None:
                    return st
                return FWD   # systematic tour during the window
            return self._explore(w)
        # crisis active
        target_et = NEED_TO_ETYPE[w.crisis_need]
        pl, score, _s = memory.recall(_etype_query(target_et))
        if score < 0.5:
            # recall for a few onset steps, then keep exploring
            return RECALL if w.t - w.crisis_onset < 4 else self._explore(w)
        if (self._recalled_onset != w.crisis_onset
                or (w.t - w.crisis_onset) % 4 == 0):
            # register + periodically refresh the env-visible recall so
            # _memory_proof passes and BC sees dense RECALL labels
            self._recalled_onset = w.crisis_onset
            return RECALL
        site = _payload_site(pl)
        if w.pos == site:
            return INTERACT
        return self._goto(w, site)

    # -- CTX-2: store only real (world-caused) events; go at window ------
    def _act_ctx2(self, w: CtxWorld, memory) -> int:
        s = w.spec
        if w.t < s.go_window[0]:
            et = self._perceives(w)
            if et == 1:
                st = self._store_here(w, et)
                if st is not None:
                    return st
            return FWD if s.announce_window[0] <= w.t < s.announce_window[1] else self._explore(w)
        pl, score, _s = memory.recall(_etype_query(1))
        site = _payload_site(pl) if score >= 0 else -1
        if site >= 0 and self._recalled_onset < 0:
            # register the recall through env (once at go time)
            self._recalled_onset = 1
            return RECALL
        if site >= 0 and w.pos == site:
            return INTERACT
        if site >= 0:
            return self._goto(w, site)
        return self._explore(w)

    # -- CTX-3: store the cue; fire RESP_k when elapsed≈D ----------------
    def _act_ctx3(self, w: CtxWorld, memory) -> int:
        s = w.spec
        if w.t < s.announce_window[0]:
            return NOOP                      # nothing perceivable yet
        if w.t < s.announce_window[1]:
            return STORE                     # covers the cue window
        if w.cue_delay <= w.t <= w.cue_delay + 5:
            return RESP0 + w.cue_resp        # dense window labels for BC
        if 0 < w.cue_delay - w.t <= 10:      # recall inside the pre-window
            return RECALL
        return NOOP

    # -- CTX-4: discover all sites; pre-position before crisis -----------
    def _act_ctx4(self, w: CtxWorld, memory) -> int:
        s = w.spec
        if w.crisis_need < 0 and w.t < w.crisis_t:
            if s.announce_window[0] <= w.t < s.announce_window[1]:
                et = self._perceives(w)
                st = self._store_here(w, et)
                if st is not None:
                    return st
                return FWD
            return self._explore(w)
        need = w.crisis_need if w.crisis_need >= 0 else w._pick_crisis_need()
        target_et = NEED_TO_ETYPE[need]
        pl, score, _s = memory.recall(_etype_query(target_et))
        if score < 0.5:
            # recall for a few onset steps, then keep exploring
            return RECALL if w.t - w.crisis_onset < 5 else self._explore(w)
        if (w.crisis_need >= 0
                and (self._recalled_onset != w.crisis_onset
                     or (w.t - w.crisis_onset) % 4 == 0)):
            # register + periodically refresh the env-visible recall
            self._recalled_onset = w.crisis_onset
            return RECALL
        site = _payload_site(pl)
        if w.pos == site:
            return INTERACT if w.crisis_need >= 0 else NOOP
        return self._goto(w, site)

    # -- CTX-5: at probes, pick the argmax drive option ------------------
    def _act_ctx5(self, w: CtxWorld, memory) -> int:
        s = w.spec
        if w.t in s.probe_times:
            return RESP0 + int(np.argmax(w.probe_options()))
        if s.announce_window[0] <= w.t < s.announce_window[1]:
            et = self._perceives(w)
            st = self._store_here(w, et)
            if st is not None:
                return st
        if w.internal[FATIGUE] > 0.8:
            return REST
        return self._explore(w)

    def _explore(self, w: CtxWorld) -> int:
        # opportunistic self-care while wandering
        et = int(w.sites[w.pos])
        if et == 1 and w.internal[ENERGY] < 0.6:
            return INTERACT
        if et == 3 and w.internal[FATIGUE] > 0.55:
            return INTERACT
        if et == 2 and abs(w.internal[TEMP] - 0.55) > 0.2:
            return INTERACT
        return FWD if w.rng.random() < 0.7 else NOOP


def _payload_site(pl: np.ndarray) -> int:
    theta = float(np.arctan2(float(pl[5]), float(pl[6])))
    return int(round(theta * N_LOCS / (2 * np.pi))) % N_LOCS


def _etype_query(et: int) -> np.ndarray:
    q = np.zeros(6, dtype=np.float32)
    q[et] = 1.0
    return q
