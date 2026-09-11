"""Non-learned memory-gate baselines for R0.

Each baseline is a `decide(env) -> (action, answer)` callable that may
read the env's ground-truth event stream and memory state. All heuristic
gates share the same read side: on an active query they RECALL once,
then ANSWER via `env.answer_key_match` (perfect content lookup, uniform
guess when the key is absent). This isolates the *storage* policy, which
is the comparison the experiment cares about.

Storage policies:
    random     store each non-NULL event with prob p_store
    fifo       store every ITEM (relevant + distractor are indistinguishable)
    lru        fifo store rule, LRU eviction (env cfg evict_policy="lru")
    reservoir  reservoir sampling over ITEM events (evict_policy="random")
    oracle     stores only items whose key will be queried (upper bound)
"""

from __future__ import annotations

import numpy as np

from ..env.r0_env import IGNORE, STORE, RECALL, ANSWER, ITEM, NOISE, QUERY


class HeuristicGate:
    evict_policy = "fifo"

    def __init__(self, seed: int = 0):
        self.rng = np.random.default_rng(seed)

    def reset(self) -> None:
        pass

    def want_store(self, env) -> bool:
        return False

    def decide(self, env) -> tuple[int, int]:
        e = env.schedule[env.t]
        if e.kind == QUERY and env.pending is None:
            # RECALL lands after the pending query opens inside step()
            return RECALL, 0
        if env.pending is not None:
            if not env.pending["recalled"]:
                return RECALL, 0
            return ANSWER, env.answer_key_match(self.rng)
        if self.want_store(env):
            return STORE, 0
        return IGNORE, 0


class RandomGate(HeuristicGate):
    def __init__(self, p_store: float = 0.25, **kw):
        super().__init__(**kw)
        self.p_store = p_store

    def want_store(self, env) -> bool:
        return env.schedule[env.t].kind in (ITEM, NOISE) and self.rng.random() < self.p_store


class FIFOGate(HeuristicGate):
    def want_store(self, env) -> bool:
        return env.schedule[env.t].kind == ITEM


class LRUGate(FIFOGate):
    evict_policy = "lru"


class ReservoirGate(HeuristicGate):
    """Classic reservoir sampling over ITEM events; needs evict_policy=random."""
    evict_policy = "random"

    def reset(self) -> None:
        self._seen = 0

    def want_store(self, env) -> bool:
        if env.schedule[env.t].kind != ITEM:
            return False
        self._seen += 1
        if self._seen <= env.cfg.memory_slots:
            return True
        return self.rng.random() < env.cfg.memory_slots / self._seen


class OracleGate(HeuristicGate):
    """Stores exactly the items that will be queried later.

    When a queried-key ITEM collides with an active query window it stores
    first and defers the answer (the deadline leaves slack), which is why
    this gate is the upper bound.
    """

    def reset(self) -> None:
        self._queried = set()

    def want_store(self, env) -> bool:
        e = env.schedule[env.t]
        if not self._queried:
            self._queried = env._queried_key_set()
        return e.kind == ITEM and e.key in self._queried

    def decide(self, env) -> tuple[int, int]:
        e = env.schedule[env.t]
        p = env.pending
        if p is not None and e.kind == ITEM and env.t < p["deadline"]:
            if not self._queried:
                self._queried = env._queried_key_set()
            if e.key in self._queried:
                return STORE, 0
        return super().decide(env)


BASELINES = {
    "random": RandomGate,
    "fifo": FIFOGate,
    "lru": LRUGate,
    "reservoir": ReservoirGate,
    "oracle": OracleGate,
}
