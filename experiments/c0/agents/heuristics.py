"""Non-learned agents for C0 environment validation.

Each exposes reset() / decide(env) -> (action, answer) and, for the memory
heuristic, agent-side SlotMemory state that the interruption harness can
wipe (cold probe) — proving pre-restart information is causally required
without any training.
"""
from __future__ import annotations

import numpy as np

from ..env.c0_env import IGNORE, STORE, ANSWER, ITEM, QUERY
from .slot_memory import SlotMemory


class RandomAgent:
    name = "random"
    has_memory = False

    def __init__(self, seed: int = 0):
        self.rng = np.random.default_rng(seed)

    def reset(self) -> None:
        pass

    def decide(self, env) -> tuple[int, int]:
        return (int(self.rng.integers(3)),
                int(self.rng.integers(env.cfg.num_values)))


class OracleAgent:
    """Upper bound: answers every query from ground truth."""
    name = "oracle"
    has_memory = False

    def reset(self) -> None:
        pass

    def decide(self, env) -> tuple[int, int]:
        a, v = env.oracle_action(store_items=False)
        if a == IGNORE and env.pending is not None:
            # pending but past the QUERY step (shouldn't happen: oracle
            # answers immediately) -- answer again anyway
            return ANSWER, env.pending["value"]
        return a, v


class StoreAllAgent:
    """Stores every ITEM in agent-side slot memory; answers by key lookup.

    This is the no-training probe for causal dependence: with its memory
    wiped at the interruption (cold restart) it must fail post queries
    while still answering ctrl queries.
    """
    name = "storeall"
    has_memory = True

    def __init__(self, env_cfg, seed: int = 0):
        self.mem = SlotMemory(env_cfg.memory_slots, env_cfg.payload_dim,
                              env_cfg.num_keys)
        self.cfg = env_cfg
        self.rng = np.random.default_rng(seed)

    def reset(self) -> None:
        self.mem.clear()

    def cold(self) -> None:
        self.mem.clear()

    def decide(self, env) -> tuple[int, int]:
        e = env.schedule[env.t]
        if env.pending is not None:
            v = self.mem.lookup_value(env.pending["key"], self.cfg.num_values)
            if v is None:
                return ANSWER, int(self.rng.integers(self.cfg.num_values))
            return ANSWER, v
        if e.kind == ITEM:
            # agent-side memory: the write is the agent's own action
            self.mem.store(env.event_payload(), e.key)
            return STORE, 0
        return IGNORE, 0


HEURISTICS = {"random": RandomAgent, "oracle": OracleAgent,
              "storeall": StoreAllAgent}
