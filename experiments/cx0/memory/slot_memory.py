"""Fixed-capacity content-addressed slot memory for CX0.

Adapted from ``experiments/r0/memory/slot_memory.py`` (R0, verdict PASS):
fixed slots, keyed refresh-in-place, FIFO/LRU/random eviction, snapshot/restore
for causal tests. CX0 adds content-addressable retrieval: RECALL returns the
occupied slot whose key vector best matches a query vector (cosine), which is
what the R0 experiment established as the validated readout mode
(R-C3 permutation is a no-op => content-based).
"""

from __future__ import annotations

import numpy as np

EVICT_FIFO = "fifo"
EVICT_LRU = "lru"
EVICT_RANDOM = "random"


class SlotMemory:
    def __init__(self, num_slots: int, payload_dim: int, key_dim: int):
        self.num_slots = num_slots
        self.payload_dim = payload_dim
        self.key_dim = key_dim
        self.payloads = np.zeros((num_slots, payload_dim), dtype=np.float32)
        self.key_vecs = np.zeros((num_slots, key_dim), dtype=np.float32)
        self.occupied = np.zeros(num_slots, dtype=bool)
        self.insert_counter = np.zeros(num_slots, dtype=np.int64)
        self.last_used = np.zeros(num_slots, dtype=np.int64)
        self._clock = 0

    # -- writes ---------------------------------------------------------
    def store(self, payload: np.ndarray, key_vec: np.ndarray, evict: str = EVICT_FIFO) -> int:
        """Store payload keyed by key_vec; identical keys refresh in place."""
        self._clock += 1
        if np.linalg.norm(key_vec) > 0:
            existing = self._find_key(key_vec)
            if existing is not None:
                s = existing
                self.payloads[s] = payload
                self.key_vecs[s] = key_vec
                self.last_used[s] = self._clock
                return s
        free = np.flatnonzero(~self.occupied)
        s = int(free[0]) if len(free) > 0 else self._evict(evict)
        self.payloads[s] = payload
        self.key_vecs[s] = key_vec
        self.occupied[s] = True
        self.insert_counter[s] = self._clock
        self.last_used[s] = self._clock
        return s

    def _find_key(self, key_vec: np.ndarray) -> int | None:
        for s in np.flatnonzero(self.occupied):
            if np.allclose(self.key_vecs[s], key_vec, atol=1e-6):
                return int(s)
        return None

    def _evict(self, evict: str) -> int:
        occ = np.flatnonzero(self.occupied)
        if evict == EVICT_LRU:
            return int(occ[np.argmin(self.last_used[occ])])
        if evict == EVICT_RANDOM:
            return int(np.random.randint(self.num_slots))
        return int(occ[np.argmin(self.insert_counter[occ])])

    # -- reads ----------------------------------------------------------
    def recall(self, query: np.ndarray) -> tuple[np.ndarray, float, int]:
        """Return (payload, match_score, slot). score<0 => nothing occupied."""
        occ = np.flatnonzero(self.occupied)
        if len(occ) == 0 or np.linalg.norm(query) == 0:
            return np.zeros(self.payload_dim, dtype=np.float32), -1.0, -1
        q = query / (np.linalg.norm(query) + 1e-9)
        ks = self.key_vecs[occ]
        kn = ks / (np.linalg.norm(ks, axis=1, keepdims=True) + 1e-9)
        sims = kn @ q
        best = int(occ[int(np.argmax(sims))])
        score = float(np.max(sims))
        self.last_used[best] = self._clock
        return self.payloads[best].copy(), score, best

    def erase(self) -> None:
        """R-C2 control: wipe all slots."""
        self.occupied[:] = False

    def shuffle_payloads(self, rng: np.random.Generator) -> None:
        """R-C1 control: permute payloads across occupied slots."""
        occ = np.flatnonzero(self.occupied)
        if len(occ) > 1:
            self.payloads[occ] = self.payloads[occ][rng.permutation(len(occ))]

    # -- snapshots ------------------------------------------------------
    def get_state(self) -> dict:
        return dict(
            payloads=self.payloads.copy(), key_vecs=self.key_vecs.copy(),
            occupied=self.occupied.copy(), insert_counter=self.insert_counter.copy(),
            last_used=self.last_used.copy(), clock=self._clock,
        )

    def set_state(self, st: dict) -> None:
        self.payloads = st["payloads"].copy()
        self.key_vecs = st["key_vecs"].copy()
        self.occupied = st["occupied"].copy()
        self.insert_counter = st["insert_counter"].copy()
        self.last_used = st["last_used"].copy()
        self._clock = st["clock"]

    def summary(self) -> np.ndarray:
        """(n_used/num_slots, mean_age, max_last_used/clock) occupancy summary."""
        n = int(self.occupied.sum())
        if n == 0:
            return np.zeros(3, dtype=np.float32)
        ages = self._clock - self.insert_counter[self.occupied]
        return np.array(
            [n / self.num_slots, float(ages.mean()) / 100.0,
             float(self.last_used[self.occupied].max()) / max(self._clock, 1)],
            dtype=np.float32,
        )
