"""R0 learned-memory-gate environment.

An episode is a fixed-length stream of events:

    NULL      nothing happened this step
    ITEM      a (key, value) pair. Items whose key is in the hidden
              "queryable" subset are queried later (RELEVANT class);
              all other keys are never queried (DISTRACTOR class).
              The class label is NOT part of the observation.
    NOISE     random payload, never queried, distinct type bit
    QUERY     asks for the value previously paired with a key. The
              query stays active for `query_window` steps.

Actions: IGNORE, STORE, RECALL, ANSWER.

STORE writes the current event payload into slot memory (FIFO eviction,
same-key refresh). RECALL snapshots the slot payloads into the recall
buffer, which is the only place the agent can read stored *values* —
the always-visible memory summary exposes only occupancy/key/age, so an
ANSWER without a prior RECALL inside the query window is blind.
ANSWER commits a value index for the active query.

The env owns the memory so heuristic baselines and causal tests share
one implementation.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np

from ..memory.slot_memory import SlotMemory

# event types
NULL, ITEM, NOISE, QUERY = 0, 1, 2, 3
# actions
IGNORE, STORE, RECALL, ANSWER = 0, 1, 2, 3
N_ACTIONS = 4
# ground-truth info classes (instrumentation only, never observed)
CLS_RELEVANT, CLS_DISTRACTOR, CLS_NOISE = "relevant", "distractor", "noise"


@dataclass
class R0Config:
    episode_len: int = 128
    num_keys: int = 16
    num_values: int = 8
    num_queryable: int = 4          # keys 0..num_queryable-1 can be queried
    queries_per_episode: int = 4    # <= num_queryable
    num_distractors: int = 12
    delay_min: int = 16
    delay_max: int = 64
    noise_rate: float = 0.5         # fraction of free steps emitting NOISE
    memory_slots: int = 4
    query_window: int = 4
    noise_dim: int = 8
    evict_policy: str = "fifo"
    reward_correct: float = 1.0
    reward_wrong: float = -0.3
    reward_answered: float = 0.2    # any resolved answer; breaks the
                                    # all-expire local optimum without
                                    # leaking which items are relevant
    reward_recall_hit: float = 0.2  # first RECALL in a query window that
                                    # retrieves the queried key; rewards
                                    # successful memory *use*, not which
                                    # keys are relevant
    reward_expire: float = -0.25
    cost_store: float = 0.01
    cost_recall: float = 0.02
    cost_bad_answer: float = 0.1    # ANSWER with no active query
    seed: int = 0

    @property
    def payload_dim(self) -> int:
        return 4 + self.num_keys + self.num_values + self.noise_dim

    @property
    def obs_dim(self) -> int:
        d = self.payload_dim
        query = 2 + self.num_keys
        summary = self.memory_slots * (self.num_keys + 2)
        recall = self.memory_slots * d
        return d + query + summary + recall + self.memory_slots + 1


@dataclass
class _Event:
    kind: int
    key: int = -1
    value: int = -1
    cls: str = CLS_NOISE


class R0Env:
    """Single-episode environment. Reset generates a fresh schedule."""

    def __init__(self, config: R0Config):
        self.cfg = config
        self.rng = np.random.default_rng(config.seed)
        self.memory = SlotMemory(config.memory_slots, config.payload_dim, config.num_keys)
        self.t = 0
        self.schedule: list[_Event] = []
        self.pending = None            # dict(key, value, deadline, recalled)
        self.recall_buffer = np.zeros(config.memory_slots * config.payload_dim, dtype=np.float32)
        self.query_records: list[dict] = []
        self.store_log: list[dict] = []
        self.action_counts = np.zeros(N_ACTIONS, dtype=np.int64)
        self._ep_seed = 0

    # ------------------------------------------------------------------
    def reset(self, seed: int | None = None) -> np.ndarray:
        if seed is not None:
            self.rng = np.random.default_rng(seed)
        self._ep_seed = int(self.rng.integers(2**31 - 1))
        self.t = 0
        self.memory.clear()
        self.pending = None
        self.recall_buffer[:] = 0
        self.query_records = []
        self.store_log = []
        self.action_counts[:] = 0
        self.schedule = self._make_schedule(np.random.default_rng(self._ep_seed))
        return self._obs()

    def _make_schedule(self, rng: np.random.Generator) -> list[_Event]:
        c = self.cfg
        for _attempt in range(4096):
            sched = [_Event(NULL) for _ in range(c.episode_len)]
            qkeys = rng.choice(c.num_queryable, size=c.queries_per_episode, replace=False)
            items, queries = [], []
            ok = True
            for k in qkeys:
                delay = int(rng.integers(c.delay_min, c.delay_max + 1))
                hi = c.episode_len - 1 - delay
                if hi < 1:
                    ok = False
                    break
                t_item = int(rng.integers(0, hi + 1))
                items.append((int(k), t_item, int(rng.integers(c.num_values))))
                queries.append((int(k), t_item + delay))
            if not ok:
                continue
            steps = [s for _, s, _ in items] + [s for _, s in queries]
            if len(set(steps)) != len(steps):
                continue
            qsteps = sorted(s for _, s in queries)
            if any(b - a <= c.query_window for a, b in zip(qsteps, qsteps[1:])):
                continue
            for k, s, v in items:
                sched[s] = _Event(ITEM, key=k, value=v, cls=CLS_RELEVANT)
            for k, s in queries:
                sched[s] = _Event(QUERY, key=k, cls=CLS_RELEVANT)
            # distractors: same ITEM format, keys outside the queryable set
            free = [s for s, e in enumerate(sched) if e.kind == NULL]
            n_dist = min(c.num_distractors, len(free))
            for s in rng.choice(free, size=n_dist, replace=False):
                k = int(rng.integers(c.num_queryable, c.num_keys))
                sched[int(s)] = _Event(ITEM, key=k, value=int(rng.integers(c.num_values)),
                                       cls=CLS_DISTRACTOR)
            # noise on a fraction of the remaining free steps
            free = [s for s, e in enumerate(sched) if e.kind == NULL]
            for s in free:
                if rng.random() < c.noise_rate:
                    sched[s] = _Event(NOISE, cls=CLS_NOISE)
            self._items = items        # (key, t_present, value) ground truth
            self._queries = queries    # (key, t_query)
            return sched
        raise RuntimeError("could not place R0 schedule; check delay/episode_len")

    # ------------------------------------------------------------------
    def _event_vec(self, e: _Event) -> np.ndarray:
        c = self.cfg
        v = np.zeros(c.payload_dim, dtype=np.float32)
        v[e.kind] = 1.0
        off = 4
        if e.kind in (ITEM, QUERY) and e.key >= 0:
            v[off + e.key] = 1.0
        off += c.num_keys
        if e.kind == ITEM and e.value >= 0:
            v[off + e.value] = 1.0
        off += c.num_values
        if e.kind == NOISE:
            v[4:4 + c.num_keys + c.num_values] = self.rng.uniform(
                0.0, 0.05, c.num_keys + c.num_values).astype(np.float32)
            v[off:off + c.noise_dim] = self.rng.uniform(0, 1, c.noise_dim)
        return v

    def _obs(self) -> np.ndarray:
        c = self.cfg
        e = self.schedule[self.t]
        parts = [self._event_vec(e)]
        q = np.zeros(2 + c.num_keys, dtype=np.float32)
        if self.pending is not None:
            q[0] = 1.0
            q[1 + self.pending["key"]] = 1.0
            q[-1] = (self.pending["deadline"] - self.t) / max(1, c.query_window)
        parts.append(q)
        parts.append(self.memory.summary(self.t, c.episode_len))
        parts.append(self.recall_buffer)
        # per-slot match bit: does the recalled payload's key equal the
        # pending query key? Derived from already-visible buffer contents;
        # all zeros until a RECALL fills the buffer, so RECALL stays
        # causally required for value readout.
        match = np.zeros(c.memory_slots, dtype=np.float32)
        if self.pending is not None:
            D = c.payload_dim
            for s in range(c.memory_slots):
                blk = self.recall_buffer[s * D:(s + 1) * D]
                keyreg = blk[4:4 + c.num_keys]
                if keyreg.sum() > 0.9 and int(np.argmax(keyreg)) == self.pending["key"]:
                    match[s] = 1.0
        parts.append(match)
        parts.append(np.array([self.t / c.episode_len], dtype=np.float32))
        return np.concatenate(parts)

    # ------------------------------------------------------------------
    def step(self, action: int, answer: int = 0):
        c = self.cfg
        e = self.schedule[self.t]
        reward = 0.0
        info: dict = {"event": e.kind, "cls": e.cls}
        self.action_counts[action] += 1

        # a QUERY event opens a pending query before the agent acts
        if e.kind == QUERY and self.pending is None:
            true_val = next(v for k, s, v in self._items if k == e.key)
            slot = self.memory.find_key(e.key)
            off = 4 + c.num_keys
            has_val = (slot is not None
                       and self.memory.payloads[slot, off:off + c.num_values].sum() > 0)
            self.pending = {"key": e.key, "value": true_val,
                            "deadline": self.t + c.query_window - 1,
                            "recalled": False, "hit_paid": False,
                            "key_in_mem": bool(has_val)}
            self.recall_buffer[:] = 0

        if action == STORE:
            reward -= c.cost_store
            if e.kind in (ITEM, NOISE):
                slot = self.memory.store(self._event_vec(e), e.key, c.evict_policy)
                self.store_log.append({"t": self.t, "cls": e.cls, "key": e.key,
                                       "slot": slot})
        elif action == RECALL:
            reward -= c.cost_recall
            self.recall_buffer = self.memory.recall().reshape(-1)
            if self.pending is not None:
                self.pending["recalled"] = True
                if (not self.pending["hit_paid"]
                        and self.memory.find_key(self.pending["key"]) is not None):
                    reward += c.reward_recall_hit
                    self.pending["hit_paid"] = True
        elif action == ANSWER:
            if self.pending is not None:
                correct = int(answer) == self.pending["value"]
                reward += c.reward_answered + (
                    c.reward_correct if correct else c.reward_wrong)
                self._close_query(answered=True, correct=correct,
                                  recalled=self.pending["recalled"])
            else:
                reward -= c.cost_bad_answer
                info["bad_answer"] = True

        self.t += 1
        if self.pending is not None and self.t > self.pending["deadline"]:
            reward += c.reward_expire
            self._close_query(answered=False, correct=False,
                              recalled=self.pending["recalled"])
        done = self.t >= c.episode_len
        obs = self._obs() if not done else np.zeros(c.obs_dim, dtype=np.float32)
        if done:
            info["ep_stats"] = self.episode_stats()
        return obs, reward, done, info

    def _close_query(self, answered: bool, correct: bool, recalled: bool) -> None:
        p = self.pending
        useful = sum(1 for s in range(self.memory.num_slots)
                     if self.memory.occupied[s]
                     and self.memory.keys[s] in self._queried_key_set())
        occupied = self.memory.num_occupied
        self.query_records.append({
            "key": p["key"], "t_query": p["deadline"] - self.cfg.query_window + 1,
            "answered": answered, "correct": correct, "recalled": recalled,
            "key_in_mem_at_query": p["key_in_mem"],
            "useful_slots": useful, "occupied_slots": occupied,
        })
        self.pending = None
        self.recall_buffer[:] = 0

    def _queried_key_set(self) -> set:
        return {k for k, _ in self._queries}

    # heuristic answer aid: content lookup shared by all non-learned agents
    def answer_key_match(self, rng: np.random.Generator) -> int:
        if self.pending is None:
            return int(rng.integers(self.cfg.num_values))
        s = self.memory.find_key(self.pending["key"])
        if s is None:
            return int(rng.integers(self.cfg.num_values))
        off = 4 + self.cfg.num_keys
        val_slice = self.memory.payloads[s, off:off + self.cfg.num_values]
        return int(np.argmax(val_slice)) if val_slice.sum() > 0 else int(
            rng.integers(self.cfg.num_values))

    # -- causal-test operations -----------------------------------------
    def erase_memory(self) -> None:
        self.memory.clear()
        self.recall_buffer[:] = 0

    def permute_memory(self, rng: np.random.Generator) -> None:
        self.memory.permute(rng.permutation(self.memory.num_slots))
        self.recall_buffer[:] = 0

    def swap_memory(self, state: dict) -> dict:
        prev = self.memory.clone_state()
        self.memory.load_state(state)
        self.recall_buffer[:] = 0
        return prev

    # ------------------------------------------------------------------
    def episode_stats(self) -> dict:
        qr = self.query_records
        n = max(1, len(qr))
        stored_keys = set(self.memory.keys[self.memory.occupied].tolist())
        useful_keys = self._queried_key_set()
        n_useful_stores = sum(1 for s in self.store_log if s["cls"] == CLS_RELEVANT)
        n_irrel_stores = sum(1 for s in self.store_log if s["cls"] != CLS_RELEVANT)
        return {
            "queries": len(qr),
            "accuracy": sum(r["correct"] for r in qr) / n,
            "answered": sum(r["answered"] for r in qr) / n,
            "recalled": sum(r["recalled"] for r in qr) / n,
            "important_retention": sum(r["key_in_mem_at_query"] for r in qr) / n,
            "memory_efficiency": (
                sum(r["useful_slots"] for r in qr) /
                max(1, sum(r["occupied_slots"] for r in qr))),
            "stores": int(self.action_counts[STORE]),
            "recalls": int(self.action_counts[RECALL]),
            "stores_useful": n_useful_stores,
            "stores_irrelevant": n_irrel_stores,
            "store_precision": n_useful_stores / max(1, n_useful_stores + n_irrel_stores),
            "final_useful": len(stored_keys & useful_keys),
            "final_occupied": self.memory.num_occupied,
        }


class VecR0Env:
    """Batched wrapper; steps a list of envs in lockstep for PPO rollouts."""

    def __init__(self, config: R0Config, num_envs: int, seed: int):
        self.envs = []
        for i in range(num_envs):
            c = R0Config(**{**config.__dict__, "seed": seed + 1000 * i})
            self.envs.append(R0Env(c))
        self.num_envs = num_envs
        self.obs_dim = config.obs_dim

    def reset(self) -> np.ndarray:
        return np.stack([e.reset() for e in self.envs])

    def step(self, actions: np.ndarray, answers: np.ndarray):
        obs, rew, done, infos = [], [], [], []
        for e, a, ans in zip(self.envs, actions, answers):
            o, r, d, i = e.step(int(a), int(ans))
            if d:
                i["terminal_obs"] = o
                o = e.reset()          # auto-reset; episode stats in info
            obs.append(o)
            rew.append(r)
            done.append(d)
            infos.append(i)
        return (np.stack(obs), np.asarray(rew, dtype=np.float32),
                np.asarray(done, dtype=bool), infos)
