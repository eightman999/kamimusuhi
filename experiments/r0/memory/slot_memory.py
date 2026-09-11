"""Fixed-capacity slot memory for the R0 learned-memory-gate experiment.

The memory holds M fixed-length payload vectors. Writes go to a free slot or,
when full, evict by a configurable policy. Storing a payload whose key is
already present refreshes that slot in place (no duplicate slot consumed).

All state is plain numpy so episodes can snapshot/restore memory for the
causal tests (R-C1 shuffle, R-C2 erase, R-C3 permutation).
"""

from __future__ import annotations

import numpy as np

EVICT_FIFO = "fifo"   # evict the earliest inserted slot
EVICT_LRU = "lru"     # evict the least recently touched slot
EVICT_RANDOM = "random"  # evict a uniformly random slot (reservoir sampling)


class SlotMemory:
    def __init__(self, num_slots: int, payload_dim: int, num_keys: int):
        self.num_slots = num_slots
        self.payload_dim = payload_dim
        self.num_keys = num_keys
        self.payloads = np.zeros((num_slots, payload_dim), dtype=np.float32)
        # key index of the stored item, or -1 for keyless payloads (noise/null)
        self.keys = np.full(num_slots, -1, dtype=np.int64)
        self.occupied = np.zeros(num_slots, dtype=bool)
        self.insert_counter = np.zeros(num_slots, dtype=np.int64)
        self.last_used = np.zeros(num_slots, dtype=np.int64)
        self._clock = 0

    # -- writes ---------------------------------------------------------
    def store(self, payload: np.ndarray, key_idx: int, evict: str = EVICT_FIFO) -> int:
        """Store payload; returns the slot index used."""
        self._clock += 1
        if key_idx >= 0:
            existing = self.find_key(key_idx)
            if existing is not None:
                s = existing
                self.payloads[s] = payload
                self.last_used[s] = self._clock
                return s
        free = np.flatnonzero(~self.occupied)
        if len(free) > 0:
            s = int(free[0])
        else:
            s = self._evict(evict)
        self.payloads[s] = payload
        self.keys[s] = key_idx
        self.occupied[s] = True
        self.insert_counter[s] = self._clock
        self.last_used[s] = self._clock
        return s

    def _evict(self, evict: str) -> int:
        if evict == EVICT_LRU:
            return int(np.argmin(self.last_used))
        if evict == EVICT_RANDOM:
            return int(np.random.randint(self.num_slots))
        return int(np.argmin(self.insert_counter))  # fifo

    # -- reads ----------------------------------------------------------
    def find_key(self, key_idx: int):
        hits = np.flatnonzero(self.occupied & (self.keys == key_idx))
        return int(hits[0]) if len(hits) else None

    def recall(self) -> np.ndarray:
        """Return the (M, D) payload block; unoccupied rows are zero."""
        self._clock += 1
        self.last_used[self.occupied] = self._clock
        return self.payloads * self.occupied[:, None]

    def touch(self, slot: int) -> None:
        self._clock += 1
        self.last_used[slot] = self._clock

    # -- summaries ------------------------------------------------------
    def summary(self, t: int, max_age: int) -> np.ndarray:
        """Per-slot [occupied, key_onehot(num_keys), age_norm] flattened."""
        out = np.zeros((self.num_slots, self.num_keys + 2), dtype=np.float32)
        for s in range(self.num_slots):
            if not self.occupied[s]:
                continue
            out[s, 0] = 1.0
            if 0 <= self.keys[s] < self.num_keys:
                out[s, 1 + self.keys[s]] = 1.0
            out[s, -1] = min(1.0, (t - self.insert_counter[s]) / max(1, max_age))
        return out.reshape(-1)

    # -- causal-test manipulations --------------------------------------
    def clear(self) -> None:
        self.__init__(self.num_slots, self.payload_dim, self.num_keys)

    def permute(self, order: np.ndarray) -> None:
        order = np.asarray(order)
        self.payloads = self.payloads[order].copy()
        self.keys = self.keys[order].copy()
        self.occupied = self.occupied[order].copy()
        self.insert_counter = self.insert_counter[order].copy()
        self.last_used = self.last_used[order].copy()

    def clone_state(self) -> dict:
        return {
            "payloads": self.payloads.copy(),
            "keys": self.keys.copy(),
            "occupied": self.occupied.copy(),
            "insert_counter": self.insert_counter.copy(),
            "last_used": self.last_used.copy(),
            "clock": self._clock,
        }

    def load_state(self, state: dict) -> None:
        self.payloads = state["payloads"].copy()
        self.keys = state["keys"].copy()
        self.occupied = state["occupied"].copy()
        self.insert_counter = state["insert_counter"].copy()
        self.last_used = state["last_used"].copy()
        self._clock = state["clock"]

    @property
    def num_occupied(self) -> int:
        return int(self.occupied.sum())
