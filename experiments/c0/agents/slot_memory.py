"""Fixed-capacity slot memory owned by the agent (not the env).

Holds M fixed-length payload vectors (raw event vectors). Writes go to a
free slot or, when full, evict FIFO; storing a payload whose key is already
present refreshes that slot in place.

All state is plain numpy so it can be snapshotted into a persistence
artifact, shuffled, or masked by the causal tests.
"""
from __future__ import annotations

import numpy as np

EVICT_FIFO = "fifo"


class SlotMemory:
    def __init__(self, num_slots: int, payload_dim: int, num_keys: int):
        self.num_slots = num_slots
        self.payload_dim = payload_dim
        self.num_keys = num_keys
        self.payloads = np.zeros((num_slots, payload_dim), dtype=np.float32)
        self.keys = np.full(num_slots, -1, dtype=np.int64)
        self.occupied = np.zeros(num_slots, dtype=bool)
        self.insert_counter = np.zeros(num_slots, dtype=np.int64)
        self.clock = 0

    # -- writes ---------------------------------------------------------
    def store(self, payload: np.ndarray, key_idx: int) -> int:
        """Store payload; returns the slot index used."""
        self.clock += 1
        if key_idx >= 0:
            existing = self.find_key(key_idx)
            if existing is not None:
                s = existing
                self.payloads[s] = payload
                return s
        free = np.flatnonzero(~self.occupied)
        s = int(free[0]) if len(free) else int(np.argmin(self.insert_counter))
        self.payloads[s] = payload
        self.keys[s] = key_idx
        self.occupied[s] = True
        self.insert_counter[s] = self.clock
        return s

    # -- reads ----------------------------------------------------------
    def find_key(self, key_idx: int):
        hits = np.flatnonzero(self.occupied & (self.keys == key_idx))
        return int(hits[0]) if len(hits) else None

    def contents(self) -> np.ndarray:
        """(M, D) block fed to the policy input; empty rows are zero."""
        return self.payloads * self.occupied[:, None]

    def lookup_value(self, key_idx: int, num_values: int) -> int | None:
        """Programmatic content lookup used by heuristic agents only."""
        s = self.find_key(key_idx)
        if s is None:
            return None
        off = 4 + self.num_keys
        seg = self.payloads[s, off:off + num_values]
        return int(np.argmax(seg)) if seg.sum() > 0 else None

    # -- state ----------------------------------------------------------
    def clear(self) -> None:
        self.__init__(self.num_slots, self.payload_dim, self.num_keys)

    def clone_state(self) -> dict:
        return {
            "mem_payloads": self.payloads.copy(),
            "mem_keys": self.keys.copy(),
            "mem_occupied": self.occupied.copy(),
            "mem_insert": self.insert_counter.copy(),
            "mem_clock": np.int64(self.clock),
        }

    def load_state(self, state: dict) -> None:
        self.payloads = state["mem_payloads"].astype(np.float32).copy()
        self.keys = state["mem_keys"].astype(np.int64).copy()
        self.occupied = state["mem_occupied"].astype(bool).copy()
        self.insert_counter = state["mem_insert"].astype(np.int64).copy()
        self.clock = int(np.asarray(state["mem_clock"]).item())

    def permute_slots(self, order: np.ndarray) -> None:
        order = np.asarray(order)
        self.payloads = self.payloads[order].copy()
        self.keys = self.keys[order].copy()
        self.occupied = self.occupied[order].copy()
        self.insert_counter = self.insert_counter[order].copy()

    @property
    def num_occupied(self) -> int:
        return int(self.occupied.sum())
