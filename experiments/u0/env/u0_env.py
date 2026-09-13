"""U0 need-guided memory environment.

An episode is a fixed-length life stream on a num_locations ring
(default 12, labelled A-L):

  * Early on, the agent *observes events*: truthful reports of service
    sites ("RESOURCE at C, potency 1.0"), plus DISTRACTOR and NOISE
    items. Whether a report will ever matter is NOT labeled.
  * Later, one or two internal variables enter a *need crisis*: an
    onset shock pushes the variable toward its lethal bound and it
    keeps drifting there until resolved or death. Which variable
    crises is sampled per episode, weighted by how far that variable
    already sits outside its preferred range at reset: a
    currently-depleted variable is the most likely future need, so
    the value of storing a given event is conditioned on the agent's
    own internal state.
  * After the event window the current observation carries NO
    location->function information: post-onset the agent may know
    *which* variable crises (its internal channels) but not *where*
    the serving site is. To resolve a crisis the agent must RECALL
    the site serving the matching function from slot memory, MOVE to
    its location, and ACT. Trial-and-error wandering exists but the
    needed potent site is one of twelve locations inside a short crisis
    budget, so a memoryless agent faces principled uncertainty.

Sites: each of the 4 functions (resource->energy, shelter->temperature,
safe_zone->risk, obs_point->certainty) is installed at TWO distinct
locations per episode, one potent (1.0) and one meager (0.35) site.
Eight useful event reports therefore exceed the 4 memory slots: keeping
"everything useful" is impossible and what is worth keeping depends on
anticipated need. A site resolves a crisis only if its pull brings the
variable back inside its preferred range, so a meager site is a partial
rescue, not a reliable fix — the potent site is the real target.

Actions (6): IGNORE, STORE, RECALL, WAIT, MOVE, ACT.
MOVE walks +1 while searching; once RECALL has produced a destination
(recall_valid), MOVE becomes directed — it steps the short way around
the ring toward the recalled location, since a known destination makes
locomotion purposeful. STORE/RECALL carry explicit reward costs;
MOVE/ACT/WAIT act through the internal dynamics.

reward = -homeostatic_error - memory_op_costs - survival_violation.
No signal ever marks an event as relevant; memory usefulness exists only
through later homeostatic consequences.

The env owns the memory so heuristic baselines and causal tests share one
implementation.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

from ..memory.slot_memory import SlotMemory

# ---------------------------------------------------------------------------
# Indices / names
# ---------------------------------------------------------------------------

INTERNAL_NAMES = ["energy", "temperature", "risk", "certainty"]
FUNCTION_NAMES = ["resource", "shelter", "safe_zone", "obs_point"]
EVENT_NAMES = ["none", "resource", "shelter", "safe_zone", "obs_point",
               "distractor", "noise"]
ACTION_NAMES = ["IGNORE", "STORE", "RECALL", "WAIT", "MOVE", "ACT"]

N_INTERNAL = len(INTERNAL_NAMES)
N_FUNCTIONS = len(FUNCTION_NAMES)
N_EVENT_TYPES = len(EVENT_NAMES)
N_ACTIONS = len(ACTION_NAMES)

ENERGY, TEMPERATURE, RISK, CERTAINTY = range(N_INTERNAL)
EV_NONE, EV_RESOURCE, EV_SHELTER, EV_SAFE, EV_OBS, EV_DISTRACTOR, EV_NOISE = \
    range(N_EVENT_TYPES)
IGNORE, STORE, RECALL, WAIT, MOVE, ACT = range(N_ACTIONS)

# ground-truth event classes (instrumentation only, never observed)
CLS_FUNCTIONAL = "functional"
CLS_DISTRACTOR = "distractor"
CLS_NOISE = "noise"
CLS_NULL = "null"

# event type id <-> function id (functional events only)
def _ev_of_func(f: int) -> int:
    return f + 1


# Preferred (homeostatic) ranges per spec section 4.
PREFERRED_RANGES = {
    "energy": (0.40, 0.80),
    "temperature": (0.35, 0.65),
    "risk": (0.00, 0.30),
    "certainty": (0.40, 1.00),
}
ERROR_WEIGHTS = {"energy": 1.0, "temperature": 1.0, "risk": 1.0,
                 "certainty": 0.7}
# Critical (lethal) thresholds; None = unbounded side. Every variable
# that can crisis must be able to kill, otherwise that need type could
# always be resolved by leisurely trial-and-error (no principled
# uncertainty) — certainty collapse counts as death for the same reason.
CRITICAL_THRESHOLDS = {
    "energy": (0.05, None),
    "temperature": (0.05, 0.95),
    "risk": (None, 0.98),
    "certainty": (0.05, None),
}

OBS_DIM = 32
PAYLOAD_DIM = 11  # event type one-hot(7) + loc sin/cos(2) + potency(1) + t(1)


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------


@dataclass
class U0Config:
    episode_len: int = 192
    num_locations: int = 12
    memory_slots: int = 3     # 4 useful functions > 3 slots: coverage of
                            # every function is impossible — selection is
                            # forced and the need-conditioned choice of
                            # WHICH func to hold is the experiment
    evict_policy: str = "fifo"

    # --- event schedule ---
    event_start: int = 12          # first functional event ~ this step
    event_spacing: int = 6
    event_jitter: int = 2
    n_distractors: int = 4
    noise_rate: float = 0.25       # fraction of free window steps -> NOISE
    sites_per_function: int = 2    # one potent + one meager site
    potency_rich: float = 1.0
    potency_meager: float = 0.35

    # --- need (crisis) schedule ---
    needs_per_episode: int = 2
    delay_min: int = 24            # need1 onset = last functional + U[min,max]
    delay_max: int = 80
    need_gap_min: int = 8          # need_k onset = need_{k-1} + U[min,max]
    need_gap_max: int = 24
    need_beta: float = 10.0        # softmax over deviation at reset
    need_floor: float = 0.05       # baseline weight for comfortable vars

    # --- crisis dynamics (drift toward lethal bound while active) ---
    # Rates are tuned so a crisis is a ~8-12 step emergency from the
    # post-shock onset state: long enough for recall+directed-move+act
    # (<= ~7 steps) and too short for a blind ring tour (~2 tries/site).
    crisis_shock: float = 0.15     # at onset, push the var toward its bound
    crisis_drain: float = 0.035    # energy downward rate
    crisis_cert_rate: float = 0.030
    crisis_temp_rate: float = 0.030
    crisis_risk_rate: float = 0.080
    # crisis drains accelerate with age: a crisis is a compounding
    # emergency — the recall->move->act chain (~4 steps) still fits the
    # early window, but a wanderer trying sites one at a time runs out
    crisis_accel: float = 0.12

    # --- passive internal dynamics ---
    passive_energy: float = 0.0025
    temp_relax: float = 0.02       # toward ambient
    ambient_amp: float = 0.10
    ambient_period: int = 240
    ambient_noise: float = 0.01
    risk_mean: float = 0.15
    risk_relax: float = 0.02
    risk_noise: float = 0.015
    risk_spike_prob: float = 0.008
    risk_spike_lo: float = 0.10
    risk_spike_hi: float = 0.30
    cert_mean: float = 0.55
    cert_relax: float = 0.010
    state_noise: float = 0.004

    # --- action effects on internal state ---
    ignore_energy: float = 0.001
    wait_energy: float = 0.010
    wait_temp_shelter: float = 0.03
    wait_risk: float = 0.012
    wait_cert: float = 0.004
    move_energy: float = 0.004
    act_energy: float = 0.003
    act_boost: float = 0.10        # preventive pull toward range midpoint
    act_wrong_energy: float = 0.045  # ACT that resolves nothing
    act_wrong_risk: float = 0.10     # wrong doors during an emergency

    # --- memory operation costs (reward terms, not energy) ---
    store_cost: float = 0.005
    recall_cost: float = 0.005
    # potential-based navigation shaping: a directed MOVE that reduces
    # ring distance to the recalled target earns a small bonus — policy-
    # invariant (Ng et al.) so it does not change the optimal policy, and
    # it rewards following your own recall readout, not which events are
    # worth storing
    nav_bonus: float = 0.02

    # --- reward ---
    death_penalty: float = 2.0
    death_forfeit: float = 0.05    # per remaining step, alive-bonus form

    # --- initial state ---
    init_perturb_prob: float = 0.75  # chance one var starts outside range
    init_perturb_lo: float = 0.05
    init_perturb_hi: float = 0.18
    # TRAINING AID ONLY — must be 0 for any reported run. With this
    # probability, a newly-active need's potent site is injected into
    # memory at onset (a mechanics scaffold: it teaches the
    # recall->move->act chain and lets the critic learn "needed func held
    # -> resolution", after which the student's own stores must create
    # that state). It never marks which *events* to store.
    prefill_need_prob: float = 0.0

    # --- causal/OOD hooks ---
    # function_shift[f] = which variable function f heals (OOD semantic
    # rotation; None = identity). Applied to sites AND recall matching.
    function_shift: tuple | None = None
    # obs_func_perm / obs_loc_perm permute the event one-hot channels
    # (U-C4: identities stay truthful, positions move) — tests whether the
    # policy memorized channel positions rather than event semantics.
    obs_func_perm: tuple | None = None
    obs_loc_perm: tuple | None = None

    seed: int = 0


# ---------------------------------------------------------------------------
# Pure helpers
# ---------------------------------------------------------------------------


def deviation(x: float, lo: float, hi: float) -> float:
    if x < lo:
        return lo - x
    if x > hi:
        return x - hi
    return 0.0


def var_deviation(internal: np.ndarray, i: int) -> float:
    lo, hi = PREFERRED_RANGES[INTERNAL_NAMES[i]]
    return deviation(float(internal[i]), lo, hi)


def homeostatic_error(internal: np.ndarray) -> float:
    """Weighted mean distance outside preferred ranges, in [0, 1]."""
    err = 0.0
    wsum = 0.0
    for i, name in enumerate(INTERNAL_NAMES):
        lo, hi = PREFERRED_RANGES[name]
        w = ERROR_WEIGHTS[name]
        err += w * deviation(float(internal[i]), lo, hi)
        wsum += w
    return err / wsum


def stable_mask(internal: np.ndarray) -> bool:
    return all(var_deviation(internal, i) == 0.0 for i in range(N_INTERNAL))


def check_death(internal: np.ndarray) -> str | None:
    for name, (lo, hi) in CRITICAL_THRESHOLDS.items():
        v = float(internal[INTERNAL_NAMES.index(name)])
        if lo is not None and v < lo:
            return name
        if hi is not None and v > hi:
            return name
    return None


@dataclass
class _Event:
    kind: int                 # EVENT_NAMES index
    func: int = -1            # function index (functional events only)
    loc: int = -1
    potency: float = 0.0
    cls: str = CLS_NULL


@dataclass
class _Need:
    var: int                  # internal variable index
    onset: int                # step at which the crisis starts
    resolved: bool = False
    active: bool = False
    retention_at_onset: bool = False
    resolve_t: int = -1
    dir: float = 0.0          # temperature crisis direction (set at onset)
    auc: float = 0.0          # homeostatic error integrated while active


# ---------------------------------------------------------------------------
# Environment
# ---------------------------------------------------------------------------


class U0Env:
    """Single-episode environment; reset() generates a fresh world."""

    def __init__(self, config: U0Config):
        self.cfg = config
        self.rng = np.random.default_rng(config.seed)
        self.memory = SlotMemory(config.memory_slots, PAYLOAD_DIM)
        self.t = 0
        self.position = 0
        self.internal = np.zeros(N_INTERNAL)
        self.ambient_phase = 0.0
        self.schedule: list[_Event] = []
        self.sites: list[dict] = []           # {func, loc, potency}
        self.needs: list[_Need] = []
        self.done = True
        self.death_cause: str | None = None
        self.recall_valid = False
        self.recall_loc = -1
        self.vuln = np.zeros(N_INTERNAL, dtype=np.float32)
        self.store_log: list[dict] = []
        self.action_counts = np.zeros(N_ACTIONS, dtype=np.int64)
        self._ep_seed = 0
        self._err_sum = 0.0
        self._err_last = 0.0
        self._stable_steps = 0
        self._func_map = (config.function_shift
                          if config.function_shift is not None
                          else tuple(range(N_FUNCTIONS)))

    # ------------------------------------------------------------------
    def reset(self, seed: int | None = None) -> np.ndarray:
        if seed is not None:
            self.rng = np.random.default_rng(seed)
        self._ep_seed = int(self.rng.integers(2**31 - 1))
        rng = np.random.default_rng(self._ep_seed)
        self.t = 0
        self.done = False
        self.death_cause = None
        self.memory.clear()
        self.recall_valid = False
        self.recall_loc = -1
        self.store_log = []
        self.action_counts[:] = 0
        self._err_sum = 0.0
        self._stable_steps = 0
        self.position = int(rng.integers(self.cfg.num_locations))
        self.ambient_phase = float(rng.uniform(0, 2 * np.pi))
        self.internal = self._initial_internal(rng)
        # vulnerability profile: the reset-time deviation the need plan
        # is sampled from. It is *constitutional* knowledge — the agent
        # may observe its own chronic weakness at any time — and is the
        # only persistent channel that predicts the coming crisis; the
        # live internal state drifts and forgets it.
        self.vuln = np.array([var_deviation(self.internal, i)
                              for i in range(N_INTERNAL)],
                             dtype=np.float32)
        self.sites = self._make_sites(rng)
        self.schedule, last_func_t = self._make_schedule(rng)
        self.needs = self._make_needs(rng, last_func_t)
        return self._obs()

    def _initial_internal(self, rng: np.random.Generator) -> np.ndarray:
        s = np.zeros(N_INTERNAL)
        for i, name in enumerate(INTERNAL_NAMES):
            lo, hi = PREFERRED_RANGES[name]
            mid = 0.5 * (lo + hi)
            half = 0.3 * (hi - lo)
            s[i] = mid + rng.uniform(-half, half)
        if rng.random() < self.cfg.init_perturb_prob:
            v = int(rng.integers(N_INTERNAL))
            push = rng.uniform(self.cfg.init_perturb_lo,
                               self.cfg.init_perturb_hi)
            lo, hi = PREFERRED_RANGES[INTERNAL_NAMES[v]]
            if INTERNAL_NAMES[v] == "risk":
                s[v] = hi + push            # risk starts too high
            elif INTERNAL_NAMES[v] == "temperature":
                s[v] = (lo - push) if rng.random() < 0.5 else (hi + push)
            else:
                s[v] = lo - push            # energy/certainty start low
        np.clip(s, 0.0, 1.0, out=s)
        return s

    def _make_sites(self, rng: np.random.Generator) -> list[dict]:
        """8 sites = 4 functions x (potent, meager); the two sites of a
        function never share a location."""
        c = self.cfg
        assert c.sites_per_function == 2, "layout assumes 2 sites/function"
        sites = []
        for f in range(N_FUNCTIONS):
            for p in (c.potency_rich, c.potency_meager):
                choices = [l for l in range(c.num_locations)
                           if not any(s["func"] == f and s["loc"] == l
                                      for s in sites)]
                loc = int(rng.choice(choices))
                sites.append({"func": f, "loc": loc, "potency": float(p)})
        return sites

    def sites_at(self, loc: int) -> list[dict]:
        return [s for s in self.sites if s["loc"] == loc]

    def _make_schedule(self, rng: np.random.Generator):
        c = self.cfg
        sched = [_Event(EV_NONE) for _ in range(c.episode_len)]
        # functional events: one report per site, shuffled order, ~fixed grid
        order = list(range(len(self.sites)))
        rng.shuffle(order)
        times = []
        last_func_t = 0
        for k, si in enumerate(order):
            t = c.event_start + k * c.event_spacing + int(
                rng.integers(-c.event_jitter, c.event_jitter + 1))
            t = max(1, min(c.episode_len - 2, t))
            s = self.sites[si]
            sched[t] = _Event(_ev_of_func(s["func"]), func=s["func"],
                              loc=s["loc"], potency=s["potency"],
                              cls=CLS_FUNCTIONAL)
            times.append(t)
        last_func_t = max(times)
        used = set(times)
        # distractors: junk reports at random locations (typed as such)
        free = [t for t in range(c.event_start, last_func_t + 1)
                if t not in used]
        for t in rng.choice(free, size=min(c.n_distractors, len(free)),
                            replace=False):
            sched[int(t)] = _Event(
                EV_DISTRACTOR, loc=int(rng.integers(c.num_locations)),
                cls=CLS_DISTRACTOR)
            used.add(int(t))
        # noise on a fraction of the remaining free window steps
        for t in range(c.event_start, last_func_t + 1):
            if t not in used and rng.random() < c.noise_rate:
                sched[t] = _Event(EV_NOISE, cls=CLS_NOISE)
        return sched, last_func_t

    def _make_needs(self, rng: np.random.Generator,
                    last_func_t: int) -> list[_Need]:
        """Sample need variables weighted by deviation *at reset*.

        The weight is need_floor + softmax-like exponential of
        need_beta * deviation, so a currently-depleted variable is the
        most likely future need — but never certain.
        """
        c = self.cfg
        devs = np.array([var_deviation(self.internal, i)
                         for i in range(N_INTERNAL)])
        # comfortable variables get only the floor weight; deviation is
        # rewarded exponentially (expm1 keeps dev=0 -> 0)
        w = c.need_floor + np.expm1(c.need_beta * devs)
        w /= w.sum()
        # needs are iid draws from the vulnerability-weighted
        # distribution; repeats are allowed — a chronically weak variable
        # can crisis twice, and one stored item then pays off twice
        n = c.needs_per_episode
        vars_ = rng.choice(N_INTERNAL, size=n, replace=True, p=w)
        needs = []
        onset = last_func_t + int(rng.integers(c.delay_min, c.delay_max + 1))
        for v in vars_:
            needs.append(_Need(var=int(v), onset=onset))
            onset += int(rng.integers(c.need_gap_min, c.need_gap_max + 1))
        # instrumentation for baselines/metrics (never in obs)
        self.need_plan = [(n.var, n.onset) for n in needs]
        return needs

    # ------------------------------------------------------------------
    def _loc_index(self, loc: int) -> int:
        """Physical location -> shown location (U-C4 permutation hook)."""
        if self.cfg.obs_loc_perm is not None and loc >= 0:
            return int(self.cfg.obs_loc_perm[loc])
        return loc

    def _event_vec(self, e: _Event) -> np.ndarray:
        v = np.zeros(PAYLOAD_DIM - 1, dtype=np.float32)
        if e.kind == EV_NONE:
            return v
        v[e.kind] = 1.0
        off = N_EVENT_TYPES
        if e.loc >= 0:
            v[off:off + 2] = self._pos_vec(self._loc_index(e.loc))
        v[-1] = e.potency
        # U-C4: permute the *channels* the agent reads, keeping semantics
        if self.cfg.obs_func_perm is not None and 1 <= e.kind <= 4:
            permuted = np.zeros_like(v)
            permuted[_ev_of_func(int(self.cfg.obs_func_perm[e.func]))] = 1.0
            permuted[off:-1] = v[off:-1]
            permuted[-1] = v[-1]
            v = permuted
        return v

    def _pos_vec(self, pos: int) -> np.ndarray:
        ang = 2.0 * np.pi * pos / self.cfg.num_locations
        return np.array([np.sin(ang), np.cos(ang)], dtype=np.float32)

    def _obs(self) -> np.ndarray:
        c = self.cfg
        e = self.schedule[self.t] if self.t < len(self.schedule) \
            else _Event(EV_NONE)
        active = [n for n in self.needs if n.active and not n.resolved]
        crisis_age = (self.t - active[0].onset) / c.episode_len \
            if active else 0.0
        # best stored potency per function (0 when absent). The agent's
        # own memory contents are internal state — the critic must be
        # able to see "needed func held" for the store->outcome credit
        # assignment to exist at all. Locations still require RECALL.
        stored_pot = np.zeros(N_FUNCTIONS, dtype=np.float32)
        occ = (self.memory.occupied & (self.memory.funcs >= 0)
               & (self.memory.funcs < N_FUNCTIONS))
        if occ.any():
            np.maximum.at(stored_pot, self.memory.funcs[occ],
                          self.memory.potencies[occ])
        parts = [
            self.internal.astype(np.float32),                    # 4
            self.vuln,                                           # 4
            stored_pot,                                          # 4
            self._pos_vec(self.position),                        # 2
            self._event_vec(e),                                  # 10
            np.array([self.memory.num_occupied / c.memory_slots],
                     dtype=np.float32),                          # 1
            np.array([float(bool(active))], dtype=np.float32),   # 1
            np.array([float(self.recall_valid)], dtype=np.float32),  # 1
            self._pos_vec(self.recall_loc) if self.recall_loc >= 0
            else np.zeros(2, dtype=np.float32),                  # 2
            np.array([len(active) / 2.0], dtype=np.float32),     # 1
            np.array([crisis_age], dtype=np.float32),            # 1
            np.array([self.t / c.episode_len], dtype=np.float32),    # 1
        ]
        return np.concatenate(parts)

    # ------------------------------------------------------------------
    def _needed_function(self) -> int | None:
        """Function whose site would resolve the *current* most urgent need:
        the earliest active crisis's variable mapped through func_map, or
        the argmax-deviation variable when no crisis is active."""
        active = [n for n in self.needs if n.active and not n.resolved]
        if active:
            var = active[0].var
        else:
            var = int(np.argmax([var_deviation(self.internal, i)
                                 for i in range(N_INTERNAL)]))
        f = self._func_of_var(var)
        return f if f >= 0 else None

    def _do_recall(self) -> None:
        f = self._needed_function()
        hit = self.memory.best_for_function(f) if f is not None else None
        self.recall_valid = hit is not None
        self.recall_loc = hit[1] if hit is not None else -1

    # ------------------------------------------------------------------
    def step(self, action: int):
        if self.done:
            raise RuntimeError("step() on finished episode; call reset()")
        c = self.cfg
        e = self.schedule[self.t]
        reward = 0.0
        info: dict = {"event": e.kind, "cls": e.cls}
        self.action_counts[action] += 1

        # need onsets fire before the action resolves this step
        for n in self.needs:
            if not n.active and not n.resolved and self.t >= n.onset:
                n.active = True
                # onset shock: the need announces itself by pushing its
                # variable partway toward the lethal bound. This keeps the
                # crisis budget (~steps until death) roughly uniform across
                # need types instead of depending on pre-crisis drift.
                if n.var == TEMPERATURE and n.dir == 0.0:
                    n.dir = 1.0 if self.internal[TEMPERATURE] >= 0.5 \
                        else -1.0
                lo_c, hi_c = CRITICAL_THRESHOLDS[INTERNAL_NAMES[n.var]]
                bound = lo_c if lo_c is not None else hi_c
                if n.var == TEMPERATURE:
                    bound = hi_c if n.dir > 0 else lo_c
                self.internal[n.var] += c.crisis_shock * (
                    bound - self.internal[n.var])
                if self.rng.random() < c.prefill_need_prob:
                    f = self._func_of_var(n.var)
                    potent = [s for s in self.sites
                              if s["func"] == f and s["potency"] > 0.9]
                    if potent:
                        b = potent[0]
                        self.memory.store(
                            self._event_vec_raw(_Event(
                                _ev_of_func(f), loc=b["loc"],
                                potency=b["potency"])),
                            f, b["loc"], b["potency"], c.evict_policy)
                n.retention_at_onset = (
                    self.memory.best_for_function(
                        self._func_of_var(n.var)) is not None)

        # --- memory operations ---
        if action == STORE:
            if e.kind != EV_NONE:
                reward -= c.store_cost
                payload = np.concatenate(
                    [self._event_vec_raw(e),
                     np.array([self.t / c.episode_len], np.float32)])
                slot = self.memory.store(
                    payload, e.func if e.cls == CLS_FUNCTIONAL else e.kind,
                    e.loc, e.potency, c.evict_policy)
                self.store_log.append({"t": self.t, "cls": e.cls,
                                       "func": e.func, "loc": e.loc,
                                       "potency": e.potency, "slot": slot})
        elif action == RECALL:
            reward -= c.recall_cost
            self._do_recall()

        # --- locomotion / interaction ---
        if action == MOVE:
            if self.recall_valid and self.recall_loc >= 0:
                # directed walk: a known destination makes locomotion
                # purposeful — step the short way around the ring, briskly
                def _ring_dist(a, b):
                    d = (a - b) % c.num_locations
                    return min(d, c.num_locations - d)
                before = _ring_dist(self.position, self.recall_loc)
                diff = (self.recall_loc - self.position) % c.num_locations
                if diff != 0:
                    step_dir = 1 if diff <= c.num_locations / 2 else -1
                    hop = min(3, diff if step_dir > 0
                              else c.num_locations - diff)
                    self.position = (self.position + step_dir * hop) \
                        % c.num_locations
                else:
                    self.position = (self.position + 1) % c.num_locations
                reward += c.nav_bonus * (
                    before - _ring_dist(self.position, self.recall_loc))
            else:
                self.position = (self.position + 1) % c.num_locations
        elif action == ACT:
            self._do_act(info)

        # --- internal dynamics ---
        self._update_internal(action)

        self.t += 1
        err = homeostatic_error(self.internal)
        self._err_sum += err
        self._err_last = err
        # crisis-error integral: every step an unresolved need stays
        # active contributes its post-update error to that need's AUC
        for n in self.needs:
            if n.active and not n.resolved:
                n.auc += err
        self._stable_steps += int(stable_mask(self.internal))
        reward -= err

        cause = check_death(self.internal)
        if cause is not None:
            self.done = True
            self.death_cause = cause
            reward -= c.death_penalty + c.death_forfeit * (
                c.episode_len - self.t)
        elif self.t >= c.episode_len:
            self.done = True
        if self.done:
            info["ep_stats"] = self.episode_stats()
        info["homeostatic_error"] = err
        info["t"] = self.t
        return self._obs() if not self.done else np.zeros(
            OBS_DIM, dtype=np.float32), float(reward), self.done, info

    def _func_of_var(self, var: int) -> int:
        """Function index that heals `var` under the (possibly shifted) map."""
        for f in range(N_FUNCTIONS):
            if self._func_map[f] == var:
                return f
        return -1

    def _event_vec_raw(self, e: _Event) -> np.ndarray:
        """Unpermuted payload written to memory (ground truth, not obs)."""
        v = np.zeros(PAYLOAD_DIM - 1, dtype=np.float32)
        v[e.kind] = 1.0
        if e.loc >= 0:
            v[N_EVENT_TYPES:N_EVENT_TYPES + 2] = self._pos_vec(e.loc)
        v[-1] = e.potency
        return v

    def _do_act(self, info: dict) -> None:
        c = self.cfg
        local = self.sites_at(self.position)
        active = [n for n in self.needs if n.active and not n.resolved]
        if active:
            # resolve the first active crisis whose variable is served by a
            # site at this location
            for n in active:
                want_func = self._func_of_var(n.var)
                match = [s for s in local if s["func"] == want_func]
                if match:
                    best = max(match, key=lambda s: s["potency"])
                    lo, hi = PREFERRED_RANGES[INTERNAL_NAMES[n.var]]
                    mid = 0.5 * (lo + hi)
                    self.internal[n.var] += best["potency"] * (
                        mid - self.internal[n.var])
                    if var_deviation(self.internal, n.var) <= 0.0:
                        n.resolved = True
                        n.active = False
                        n.resolve_t = self.t
                        info["resolved_need"] = n.var
                    else:
                        # a weak site gives partial relief but no
                        # resolution — the agent must re-ACT or find the
                        # potent site. Not penalized as a wrong door.
                        info["partial_act"] = True
                    return
            self.internal[ENERGY] -= c.act_wrong_energy
            self.internal[RISK] += c.act_wrong_risk
            info["failed_act"] = True
        else:
            if not local:
                self.internal[ENERGY] -= c.act_wrong_energy
                info["failed_act"] = True
                return
            # preventive use: each local site nudges its variable toward
            # the preferred midpoint — a weak form of crisis resolution
            # that can never push a variable out of range
            for s in local:
                var = self._func_map[s["func"]]
                lo, hi = PREFERRED_RANGES[INTERNAL_NAMES[var]]
                mid = 0.5 * (lo + hi)
                self.internal[var] += c.act_boost * s["potency"] * (
                    mid - self.internal[var])
            info["act_sites"] = len(local)

    def _update_internal(self, action: int) -> None:
        c = self.cfg
        s = self.internal
        rng = self.rng
        # passive drift
        s[ENERGY] -= c.passive_energy
        ambient = (0.5 + c.ambient_amp *
                   np.sin(2 * np.pi * self.t / c.ambient_period
                          + self.ambient_phase)
                   + rng.normal(0.0, c.ambient_noise))
        s[TEMPERATURE] += c.temp_relax * (ambient - s[TEMPERATURE])
        s[RISK] += c.risk_relax * (c.risk_mean - s[RISK]) + \
            rng.normal(0.0, c.risk_noise)
        if rng.random() < c.risk_spike_prob:
            s[RISK] += rng.uniform(c.risk_spike_lo, c.risk_spike_hi)
        s[CERTAINTY] += c.cert_relax * (c.cert_mean - s[CERTAINTY])
        # action modifiers
        if action == IGNORE:
            s[ENERGY] -= c.ignore_energy
        elif action == WAIT:
            s[ENERGY] += c.wait_energy
            s[TEMPERATURE] += c.wait_temp_shelter * (0.5 - s[TEMPERATURE])
            s[RISK] -= c.wait_risk
            s[CERTAINTY] += c.wait_cert
        elif action == MOVE:
            s[ENERGY] -= c.move_energy
        elif action == ACT:
            s[ENERGY] -= c.act_energy
        # active crises drive their variable toward the lethal bound,
        # accelerating with crisis age — an unresolved emergency compounds
        for n in self.needs:
            if not n.active or n.resolved:
                continue
            accel = 1.0 + c.crisis_accel * max(0, self.t - n.onset)
            if n.var == ENERGY:
                s[ENERGY] -= c.crisis_drain * accel
            elif n.var == CERTAINTY:
                s[CERTAINTY] -= c.crisis_cert_rate * accel
            elif n.var == RISK:
                s[RISK] += c.crisis_risk_rate * accel
            elif n.var == TEMPERATURE:
                if n.dir == 0.0:
                    n.dir = 1.0 if s[TEMPERATURE] >= 0.5 else -1.0
                s[TEMPERATURE] += n.dir * c.crisis_temp_rate * accel
        s += rng.normal(0.0, c.state_noise, size=N_INTERNAL)
        np.clip(s, 0.0, 1.0, out=s)

    # -- causal-test operations -----------------------------------------
    def erase_memory(self) -> None:
        self.memory.clear()
        self.recall_valid = False
        self.recall_loc = -1

    def erase_for_need(self, var: int) -> int:
        """Targeted erase (U-C1b / U-H4b): drop only the stored items
        whose function serves `var` — the memories the agent stored
        *for this need*. Returns the number of slots removed."""
        f = self._func_of_var(var)
        n = self.memory.remove_func(f) if f >= 0 else 0
        self.recall_valid = False
        self.recall_loc = -1
        return n

    def swap_memory(self, state: dict) -> dict:
        prev = self.memory.clone_state()
        self.memory.load_state(state)
        self.recall_valid = False
        self.recall_loc = -1
        return prev

    # ------------------------------------------------------------------
    def episode_stats(self) -> dict:
        planned = list(self.needs)
        fired = [n for n in planned if n.active or n.resolved]
        died = self.death_cause is not None
        useful_funcs = {self._func_of_var(n.var) for n in planned}
        stores = [s for s in self.store_log]
        n_func = sum(1 for s in stores if s["cls"] == CLS_FUNCTIONAL)
        n_useful = sum(1 for s in stores
                       if s["cls"] == CLS_FUNCTIONAL
                       and s["func"] in useful_funcs)
        # Per-need crisis AUC: homeostatic error integrated from onset to
        # resolution / death / episode end. Steps the agent did not live
        # (death before resolution, or before the need even fired) are
        # forfeited at the error measured at death, mirroring error_full,
        # so an early death cannot shrink the integral.
        aucs = []
        for n in planned:
            remaining = max(0, self.cfg.episode_len - max(self.t, n.onset))
            aucs.append(n.auc + (self._err_last * remaining
                                 if not n.resolved else 0.0))
        ttr = [(n.resolve_t - n.onset) if n.resolved
               else (self.cfg.episode_len - n.onset) for n in planned]
        survived = [1.0 if (n.resolved or not died) else 0.0
                    for n in planned]
        return {
            "mean_error": self._err_sum / max(1, self.t),
            "error_full": (self._err_sum + self._err_last *
                           max(0, self.cfg.episode_len - self.t))
                          / self.cfg.episode_len,
            "survival_fraction": self.t / self.cfg.episode_len,
            "died": self.death_cause is not None,
            "death_cause": self.death_cause,
            "stable_fraction": self._stable_steps / max(1, self.t),
            "needs_planned": len(planned),
            "needs_fired": len(fired),
            "needs_resolved": sum(n.resolved for n in planned),
            "need_resolution": (sum(n.resolved for n in planned)
                                / max(1, len(planned))),
            "crisis_error_auc": float(np.mean(aucs)) if aucs else 0.0,
            "time_to_resolution": float(np.mean(ttr)) if ttr else 0.0,
            "survival_after_need": float(np.mean(survived))
                                   if survived else 1.0,
            "important_retention": (
                sum(n.retention_at_onset for n in fired)
                / max(1, len(fired))),
            "stores": int(self.action_counts[STORE]),
            "stores_functional": n_func,
            "stores_useful": n_useful,
            "stores_junk": len(stores) - n_func,
            "store_precision": n_useful / max(1, len(stores)),
            "recalls": int(self.action_counts[RECALL]),
            "moves": int(self.action_counts[MOVE]),
            "acts": int(self.action_counts[ACT]),
            "memory_occupied": self.memory.num_occupied,
        }


class VecU0Env:
    """Batched wrapper stepping a list of envs in lockstep for PPO.

    Does NOT auto-reset: a finished env stays done and emits zero obs /
    zero reward so rollouts are exactly one episode per env (possibly
    padded after an early death), which keeps recurrent hidden states
    episode-aligned.
    """

    def __init__(self, config: U0Config, num_envs: int, seed: int):
        self.envs = [U0Env(U0Config(**{**config.__dict__,
                                       "seed": seed + 1000 * i}))
                     for i in range(num_envs)]
        self.num_envs = num_envs
        self.obs_dim = OBS_DIM

    def reset(self) -> np.ndarray:
        return np.stack([e.reset() for e in self.envs])

    def step(self, actions: np.ndarray):
        obs, rew, done, infos = [], [], [], []
        for e, a in zip(self.envs, actions):
            if e.done:
                obs.append(np.zeros(self.obs_dim, dtype=np.float32))
                rew.append(0.0)
                done.append(True)
                infos.append({})
                continue
            o, r, d, i = e.step(int(a))
            obs.append(o)
            rew.append(r)
            done.append(d)
            infos.append(i)
        return (np.stack(obs), np.asarray(rew, dtype=np.float32),
                np.asarray(done, dtype=bool), infos)
