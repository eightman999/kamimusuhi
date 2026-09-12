"""Fixed-capacity slot memory for the U0 need-guided memory experiment.

Holds M event payloads. Each stored item records which function it serves
(``funcs``), where the site is (``locs``) and how potent it is
(``potencies``), so the recall readout can answer "where is the best site
for the currently needed function". Writes go to a free slot or, when
full, evict by a configurable policy. Storing an event whose
(function, location) pair is already present refreshes that slot in
place (no duplicate slot consumed).

All state is plain numpy so episodes can snapshot/restore memory for the
causal tests (U-C1 erase, U-C2 shuffle, U-C4 permutation).
"""

from __future__ import annotations

import numpy as np

EVICT_FIFO = "fifo"    # evict the earliest inserted slot
EVICT_LRU = "lru"      # evict the least recently touched slot
EVICT_RANDOM = "random"  # evict a uniformly random occupied slot


class SlotMemory:
    def __init__(self, num_slots: int, payload_dim: int):
        self.num_slots = num_slots
        self.payload_dim = payload_dim
        self.payloads = np.zeros((num_slots, payload_dim), dtype=np.float32)
        self.funcs = np.full(num_slots, -1, dtype=np.int64)
        self.locs = np.full(num_slots, -1, dtype=np.int64)
        self.potencies = np.zeros(num_slots, dtype=np.float32)
        self.occupied = np.zeros(num_slots, dtype=bool)
        self.insert_counter = np.zeros(num_slots, dtype=np.int64)
        self.last_used = np.zeros(num_slots, dtype=np.int64)
        self._clock = 0

    # -- writes ---------------------------------------------------------
    def store(self, payload: np.ndarray, func: int, loc: int,
              potency: float, evict: str = EVICT_FIFO) -> int:
        """Store payload; returns the slot index used."""
        self._clock += 1
        if func >= 0:
            existing = self.find(func, loc)
            if existing is not None:
                s = existing
                self.payloads[s] = payload
                self.potencies[s] = potency
                self.last_used[s] = self._clock
                return s
        free = np.flatnonzero(~self.occupied)
        if len(free) > 0:
            s = int(free[0])
        else:
            s = self._evict(evict)
        self.payloads[s] = payload
        self.funcs[s] = func
        self.locs[s] = loc
        self.potencies[s] = potency
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
    def find(self, func: int, loc: int):
        hits = np.flatnonzero(
            self.occupied & (self.funcs == func) & (self.locs == loc))
        return int(hits[0]) if len(hits) else None

    def best_for_function(self, func: int):
        """(slot, loc) of the most potent stored site serving `func`."""
        hits = np.flatnonzero(self.occupied & (self.funcs == func))
        if not len(hits):
            return None
        s = int(hits[np.argmax(self.potencies[hits])])
        return s, int(self.locs[s])

    def recall(self) -> np.ndarray:
        """Return the (M, D) payload block; unoccupied rows are zero."""
        self._clock += 1
        self.last_used[self.occupied] = self._clock
        return self.payloads * self.occupied[:, None]

    # -- causal-test manipulations --------------------------------------
    def clear(self) -> None:
        self.__init__(self.num_slots, self.payload_dim)

    def permute(self, order: np.ndarray) -> None:
        order = np.asarray(order)
        for attr in ("payloads", "funcs", "locs", "potencies", "occupied",
                     "insert_counter", "last_used"):
            setattr(self, attr, getattr(self, attr)[order].copy())

    def clone_state(self) -> dict:
        return {k: getattr(self, k).copy() for k in
                ("payloads", "funcs", "locs", "potencies", "occupied",
                 "insert_counter", "last_used")} | {"clock": self._clock}

    def load_state(self, state: dict) -> None:
        for k in ("payloads", "funcs", "locs", "potencies", "occupied",
                  "insert_counter", "last_used"):
            setattr(self, k, state[k].copy())
        self._clock = state["clock"]

    @property
    def num_occupied(self) -> int:
        return int(self.occupied.sum())
