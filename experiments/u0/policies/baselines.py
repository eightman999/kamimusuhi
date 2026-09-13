"""Non-learned policies for U0.

Every heuristic shares the same recovery routine — during a crisis it
RECALLs once, MOVEs toward the recalled site and ACTs there; if the
memory holds nothing for the active need it wanders the ring and tries
each location (the no-memory fallback). The policies differ ONLY in
their storage rule, isolating memory-gate quality the same way R0 did.

Storage rules:
    random                  store any event with prob p_store
    fifo                    store every functional event (FIFO eviction)
    lru                     fifo rule + LRU eviction
    store_all               store every non-NULL event, junk included
    heuristic_current_need  store iff the event's variable is currently
                            outside its preferred range (myopic: reads
                            present need, does not anticipate future need)
    no_memory               never stores/recalls — current observation
                            only; during crisis it wanders and tries each
                            location once (the U0 lower bound)
    oracle                  stores exactly the potent site of each
                            function that will actually be needed
"""

from __future__ import annotations

import numpy as np

from ..env.u0_env import (ACT, EV_NONE, IGNORE, MOVE, RECALL,
                          STORE, WAIT, CLS_FUNCTIONAL, var_deviation)
from .base import Policy


def _housekeep(env) -> int:
    """Cheap homeostatic housekeeping shared by all heuristics."""
    s = env.internal
    temp = s[1]
    if temp < 0.38 or temp > 0.62 or s[2] > 0.42 or s[0] < 0.35:
        return WAIT
    return IGNORE


class HeuristicPolicy(Policy):
    """Shared crisis-recovery skeleton; subclasses override want_store."""

    evict_policy = "fifo"

    def __init__(self, seed: int = 0):
        self.rng = np.random.default_rng(seed)
        self.reset()

    def reset(self) -> None:
        self._recalled_for: int | None = None
        self._tried: set[int] = set()

    def want_store(self, env) -> bool:
        return False

    def decide(self, env) -> int:
        active = [n for n in env.needs if n.active and not n.resolved]
        if active:
            if self._recalled_for != active[0].var:
                # re-query memory for *this* crisis; the readout from a
                # previous crisis is stale and must not be trusted
                self._recalled_for = active[0].var
                self._tried = set()
                return RECALL
            if env.recall_valid and env.recall_loc >= 0:
                return ACT if env.position == env.recall_loc else MOVE
            # memory has nothing for this need: wander and try sites
            if env.position not in self._tried:
                self._tried.add(env.position)
                return ACT
            return MOVE
        self._recalled_for = None
        self._tried = set()
        e = env.schedule[env.t]
        if e.kind != EV_NONE and self.want_store(env):
            return STORE
        return _housekeep(env)


class RandomPolicy(HeuristicPolicy):
    name = "random"

    def __init__(self, p_store: float = 0.2, **kw):
        super().__init__(**kw)
        self.p_store = p_store

    def want_store(self, env) -> bool:
        return self.rng.random() < self.p_store


class FIFOPolicy(HeuristicPolicy):
    name = "fifo"

    def want_store(self, env) -> bool:
        return env.schedule[env.t].cls == CLS_FUNCTIONAL


class LRUPolicy(FIFOPolicy):
    name = "lru"
    evict_policy = "lru"


class StoreAllPolicy(HeuristicPolicy):
    name = "store_all"

    def want_store(self, env) -> bool:
        return True


class CurrentNeedPolicy(HeuristicPolicy):
    """Stores events for variables that hurt *right now*.

    Myopic: it consults the current deviation only and never anticipates
    which variable will crisis later. Because needs are sampled from the
    reset-time deviation profile it sometimes coincides with the oracle —
    and sometimes deviates when the state drifts between reset and the
    event window.
    """

    name = "heuristic_current_need"
    dev_threshold = 0.08

    def want_store(self, env) -> bool:
        e = env.schedule[env.t]
        if e.cls != CLS_FUNCTIONAL:
            return False
        var = env._func_map[e.func]
        return var_deviation(env.internal, var) > self.dev_threshold


class NoMemoryPolicy(HeuristicPolicy):
    """Lower bound: never stores, never recalls, acts on the current
    observation only. During a crisis it wanders the ring and tries each
    location once — with no memory the location->function mapping is
    unknowable, so this is a slow blind search under a death clock.
    This is the U0 lower bound the learned policy must beat."""

    name = "no_memory"

    def want_store(self, env) -> bool:
        return False

    def decide(self, env) -> int:
        active = [n for n in env.needs if n.active and not n.resolved]
        if active:
            if env.position not in self._tried:
                self._tried.add(env.position)
                return ACT
            return MOVE
        self._tried = set()
        return _housekeep(env)


class OraclePolicy(HeuristicPolicy):
    """Upper bound: stores the potent site of every to-be-needed function."""

    name = "oracle"

    def reset(self) -> None:
        super().reset()
        self._needed_funcs: set[int] = set()

    def want_store(self, env) -> bool:
        e = env.schedule[env.t]
        if e.cls != CLS_FUNCTIONAL:
            return False
        if not self._needed_funcs:
            self._needed_funcs = {
                env._func_of_var(v) for v, _onset in env.need_plan}
        return (e.func in self._needed_funcs
                and e.potency >= env.cfg.potency_rich)


BASELINES = {
    "random": RandomPolicy,
    "fifo": FIFOPolicy,
    "lru": LRUPolicy,
    "store_all": StoreAllPolicy,
    "heuristic_current_need": CurrentNeedPolicy,
    "no_memory": NoMemoryPolicy,
    "oracle": OraclePolicy,
}
