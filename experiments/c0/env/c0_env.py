"""C0 environment: continuity across interruption.

A single-episode sequence environment. The schedule is fixed by the episode
seed and split by *interruption points* into intervals:

    interval 0: [0, stop_0)              -- agent present
    gap i:      [stop_i, stop_i + gap_i) -- agent absent (process stopped)
    interval i: [resume_i, stop_{i+1})   -- agent present
    last:       [resume_{K-1}, T)

Event types: NULL, ITEM(key->value), NOISE, QUERY(key).
Actions:     IGNORE, STORE, ANSWER (value via the answer head).

Dependency classes (ground truth instrumentation, never observed):

    pre   item and query both inside interval 0            (crossed = 0)
    post  item shown before some stop, query opens after   (crossed >= 1)
          the matching resume: answering needs information
          carried across the process boundary
    ctrl  item and query both inside the last interval     (crossed = 0)
          post-restart control: shows the agent still works
    mid   items placed in a middle interval (multi-stop
          episodes), queried in the last interval

The post-restart observation stream never re-shows a crossed item, so a
stateless probe can still answer ctrl queries but must fail post queries.
"""
from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np

# event types
NULL, ITEM, NOISE, QUERY = 0, 1, 2, 3
# actions
IGNORE, STORE, ANSWER = 0, 1, 2
N_ACTIONS = 3
# dependency classes
CLS_PRE, CLS_POST, CLS_CTRL, CLS_DISTRACTOR = "pre", "post", "ctrl", "distractor"


@dataclass
class C0Config:
    episode_len: int = 160
    num_keys: int = 8
    num_values: int = 8
    num_queryable: int = 6          # keys 0..num_queryable-1 may be queried
    n_pre: int = 1                  # pre-stop item+query pairs
    n_bridge: int = 2               # interval-0 items, last-interval queries
    n_mid: int = 0                  # middle-interval items, last-interval queries
    n_ctrl: int = 2                 # last-interval item+query pairs
    stops: tuple = ((80, 0),)       # (stop_step, gap) pairs; resume = stop+gap
    item_lo: int = 6                # earliest item step
    item_hi_margin: int = 8         # items in interval 0 end at stop-margin
    pre_delay_min: int = 12         # min item->query lag, pre queries
    post_q_lo: int = 20             # bridge/mid queries start >= last_resume+this
    ctrl_item_lo: int = 4           # ctrl items >= last_resume+this
    ctrl_item_hi: int = 44          # ctrl items <= last_resume+this
    ctrl_delay_min: int = 16        # min item->query lag, ctrl queries
    num_distractors: int = 8        # ITEM events with non-queryable keys
    noise_rate: float = 0.4         # fraction of free steps emitting NOISE
    query_window: int = 4
    noise_dim: int = 8
    memory_slots: int = 16          # agent-side slot memory capacity
    reward_correct: float = 1.0
    reward_wrong: float = -0.3
    reward_answered: float = 0.2
    reward_expire: float = -0.25
    cost_store: float = 0.01
    cost_bad_answer: float = 0.1
    seed: int = 0

    # -- derived -------------------------------------------------------
    @property
    def payload_dim(self) -> int:
        return 4 + self.num_keys + self.num_values + self.noise_dim

    @property
    def obs_dim(self) -> int:
        return self.payload_dim + (2 + self.num_keys) + 1

    @property
    def resumes(self) -> tuple:
        return tuple(s + g for s, g in self.stops)

    @property
    def n_stops(self) -> int:
        return len(self.stops)

    def intervals(self) -> list[tuple[int, int]]:
        """[start, end) pairs the agent is present for."""
        out = []
        prev = 0
        for s, g in self.stops:
            out.append((prev, s))
            prev = s + g
        out.append((prev, self.episode_len))
        return out

    def gaps(self) -> list[tuple[int, int]]:
        return [(s, s + g) for s, g in self.stops if g > 0]

    def validate(self) -> None:
        prev_resume = 0
        for s, g in self.stops:
            if g < 0 or s <= prev_resume or s >= self.episode_len - 4:
                raise ValueError(f"bad stop spec {self.stops}")
            prev_resume = s + g
        need = self.n_pre + self.n_bridge + self.n_mid + self.n_ctrl
        if need > self.num_queryable:
            raise ValueError(f"need {need} queryable keys, have {self.num_queryable}")
        n_items = need + self.num_distractors
        if n_items > self.memory_slots:
            raise ValueError("memory_slots must hold all storable items "
                             f"({n_items} > {self.memory_slots})")


@dataclass
class _Event:
    kind: int
    key: int = -1
    value: int = -1
    cls: str = ""                   # dependency class of the event


class C0Env:
    """Single-episode env. reset(seed) fixes the schedule; step() advances."""

    def __init__(self, config: C0Config):
        config.validate()
        self.cfg = config
        self._ep_seed = 0
        self.t = 0
        self.schedule: list[_Event] = []
        self.pending = None         # dict(key, value, deadline)
        self.query_records: list[dict] = []
        self.action_counts = np.zeros(N_ACTIONS, dtype=np.int64)
        self._items: list[tuple] = []    # (key, t_item, value)
        self._queries: list[tuple] = []  # (key, t_query)

    # ------------------------------------------------------------------
    def reset(self, seed: int | None = None) -> np.ndarray:
        ep_seed = self.cfg.seed if seed is None else int(seed)
        self._ep_seed = ep_seed
        self.t = 0
        self.pending = None
        self.query_records = []
        self.action_counts[:] = 0
        self.schedule = self._make_schedule(np.random.default_rng(ep_seed))
        return self._obs()

    def _make_schedule(self, rng: np.random.Generator) -> list[_Event]:
        c = self.cfg
        intervals = c.intervals()
        gaps = c.gaps()
        last_lo, _ = intervals[-1]
        stop0 = c.stops[0][0]
        for _attempt in range(8192):
            sched = [_Event(NULL) for _ in range(c.episode_len)]
            qkeys = rng.choice(c.num_queryable,
                               size=c.n_pre + c.n_bridge + c.n_mid + c.n_ctrl,
                               replace=False)
            items, queries = [], []          # (key, t_item, value), (key, t_q)
            classes = {}                     # t_query -> cls
            qi = iter(qkeys.tolist())
            # -- pre pairs: fully inside interval 0
            for _ in range(c.n_pre):
                k = next(qi)
                t_i = int(rng.integers(c.item_lo, stop0 - c.item_hi_margin))
                lo = t_i + c.pre_delay_min
                hi = stop0 - c.query_window       # deadline = t_q+win-1 < stop0
                if hi < lo:
                    break
                t_q = int(rng.integers(lo, hi + 1))
                items.append((k, t_i, int(rng.integers(c.num_values))))
                queries.append((k, t_q))
                classes[t_q] = CLS_PRE
            else:
                # -- bridge pairs: item in interval 0, query in last interval
                for _ in range(c.n_bridge):
                    k = next(qi)
                    t_i = int(rng.integers(c.item_lo, stop0 - c.item_hi_margin))
                    t_q = int(rng.integers(last_lo + c.post_q_lo,
                                           c.episode_len - 4))
                    items.append((k, t_i, int(rng.integers(c.num_values))))
                    queries.append((k, t_q))
                    classes[t_q] = CLS_POST
                # -- mid pairs: item in a middle interval, query last interval
                for j in range(c.n_mid):
                    k = next(qi)
                    ivl = intervals[1 + (j % max(1, len(intervals) - 2))]
                    t_i = int(rng.integers(ivl[0] + 2,
                                           max(ivl[0] + 3, ivl[1] - 10)))
                    t_q = int(rng.integers(last_lo + c.post_q_lo,
                                           c.episode_len - 4))
                    items.append((k, t_i, int(rng.integers(c.num_values))))
                    queries.append((k, t_q))
                    classes[t_q] = CLS_POST
                # -- ctrl pairs: fully inside the last interval
                for _ in range(c.n_ctrl):
                    k = next(qi)
                    t_i = int(rng.integers(last_lo + c.ctrl_item_lo,
                                           last_lo + c.ctrl_item_hi))
                    lo = t_i + c.ctrl_delay_min
                    hi = c.episode_len - 4
                    if hi < lo:
                        break
                    t_q = int(rng.integers(lo, hi + 1))
                    items.append((k, t_i, int(rng.integers(c.num_values))))
                    queries.append((k, t_q))
                    classes[t_q] = CLS_CTRL
                if len(queries) != len(classes):
                    continue
                steps = [s for _, s, _ in items] + [s for _, s in queries]
                if len(set(steps)) != len(steps):
                    continue
                qsteps = sorted(s for _, s in queries)
                if any(b - a <= c.query_window
                       for a, b in zip(qsteps, qsteps[1:])):
                    continue
                # nothing non-trivial may fall inside a gap
                ev_steps = set(steps)
                if any(any(g0 <= s < g1 for g0, g1 in gaps) for s in ev_steps):
                    continue
                for k, s, v in items:
                    cls = CLS_DISTRACTOR
                    for (kk, tq) in queries:
                        if kk == k:
                            cls = classes[tq]
                    sched[s] = _Event(ITEM, key=k, value=v, cls=cls)
                for k, s in queries:
                    sched[s] = _Event(QUERY, key=k, cls=classes[s])
                # distractors: same ITEM format, keys outside the queryable set
                free = [s for s, e in enumerate(sched)
                        if e.kind == NULL
                        and not any(g0 <= s < g1 for g0, g1 in gaps)]
                n_dist = min(c.num_distractors, len(free))
                for s in rng.choice(free, size=n_dist, replace=False):
                    k = int(rng.integers(c.num_queryable, c.num_keys))
                    sched[int(s)] = _Event(ITEM, key=k,
                                           value=int(rng.integers(c.num_values)),
                                           cls=CLS_DISTRACTOR)
                # noise on a fraction of the remaining free steps
                free = [s for s, e in enumerate(sched) if e.kind == NULL]
                for s in free:
                    if rng.random() < c.noise_rate:
                        sched[s] = _Event(NOISE, cls="noise")
                self._items = items
                self._queries = queries
                self._q_classes = classes
                return sched
        raise RuntimeError("could not place C0 schedule; check config")

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
            # deterministic per (episode, step): identical across phases
            r = np.random.default_rng(self._ep_seed * 1_000_003 + self.t)
            v[4:4 + c.num_keys + c.num_values] = r.uniform(
                0.0, 0.05, c.num_keys + c.num_values).astype(np.float32)
            v[off:off + c.noise_dim] = r.uniform(0, 1, c.noise_dim)
        return v

    def _obs(self) -> np.ndarray:
        c = self.cfg
        e = self.schedule[self.t]
        parts = [self._event_vec(e)]
        q = np.zeros(2 + c.num_keys, dtype=np.float32)
        if self.pending is not None:
            q[0] = 1.0
            q[1 + self.pending["key"]] = 1.0
            q[-1] = (self.pending["deadline"] - self.t + 1) / c.query_window
        parts.append(q)
        parts.append(np.array([self.t / c.episode_len], dtype=np.float32))
        return np.concatenate(parts)

    # ------------------------------------------------------------------
    def step(self, action: int, answer: int = 0):
        c = self.cfg
        if self.t >= c.episode_len:
            raise RuntimeError("episode finished")
        e = self.schedule[self.t]
        reward = 0.0
        info: dict = {"event": e.kind, "cls": e.cls, "t": self.t}
        self.action_counts[action] += 1

        # a QUERY event opens a pending query before the agent acts
        if e.kind == QUERY and self.pending is None:
            true_val = next(v for k, s, v in self._items if k == e.key)
            self.pending = {"key": e.key, "value": true_val,
                            "deadline": self.t + c.query_window - 1,
                            "t_query": self.t}

        if action == STORE:
            reward -= c.cost_store           # memory itself is agent-side
        elif action == ANSWER:
            if self.pending is not None:
                correct = int(answer) == self.pending["value"]
                reward += c.reward_answered + (
                    c.reward_correct if correct else c.reward_wrong)
                self._close_query(answered=True, correct=correct)
            else:
                reward -= c.cost_bad_answer
                info["bad_answer"] = True

        self.t += 1
        if self.pending is not None and self.t > self.pending["deadline"]:
            reward += c.reward_expire
            self._close_query(answered=False, correct=False)
        done = self.t >= c.episode_len
        obs = self._obs() if not done else np.zeros(c.obs_dim, dtype=np.float32)
        if done:
            info["ep_stats"] = self.episode_stats()
        return obs, reward, done, info

    def _close_query(self, answered: bool, correct: bool) -> None:
        p = self.pending
        t_item = next(s for k, s, v in self._items if k == p["key"])
        crossed = sum(1 for s, g in self.cfg.stops
                      if t_item < s and p["t_query"] >= s + g)
        self.query_records.append({
            "key": p["key"], "t_item": t_item, "t_query": p["t_query"],
            "cls": self._q_classes[p["t_query"]], "crossed": crossed,
            "answered": answered, "correct": correct,
        })
        self.pending = None

    # -- interruption support -------------------------------------------
    def advance_to(self, t: int) -> np.ndarray:
        """Fast-forward to a resume step; only NULL/NOISE may be skipped."""
        for s in range(self.t, t):
            if self.schedule[s].kind in (ITEM, QUERY):
                raise RuntimeError(f"gap skips event at t={s}")
        self.t = t
        return self._obs()

    def event_key(self, t: int | None = None) -> int:
        """Key index of the event at t (default now); -1 if none."""
        e = self.schedule[self.t if t is None else t]
        return e.key if e.kind in (ITEM, QUERY) else -1

    def event_payload(self, t: int | None = None) -> np.ndarray:
        return self._event_vec(self.schedule[self.t if t is None else t])

    def oracle_action(self, store_items: bool) -> tuple[int, int]:
        """Oracle (training only): ANSWER true value on a pending/QUERY step,
        STORE on ITEM events when the agent has a memory, else IGNORE."""
        e = self.schedule[self.t]
        if e.kind == QUERY:
            return ANSWER, next(v for k, s, v in self._items if k == e.key)
        if self.pending is not None:
            # query already answered or not; ANSWER once was enough
            return IGNORE, 0
        if e.kind == ITEM and store_items:
            return STORE, 0
        return IGNORE, 0

    # ------------------------------------------------------------------
    def episode_stats(self) -> dict:
        qr = self.query_records
        out = {"queries": len(qr),
               "accuracy": np.mean([r["correct"] for r in qr]) if qr else 0.0,
               "answered": np.mean([r["answered"] for r in qr]) if qr else 0.0,
               "stores": int(self.action_counts[STORE]),
               "answers": int(self.action_counts[ANSWER])}
        for cls in (CLS_PRE, CLS_POST, CLS_CTRL):
            sel = [r for r in qr if r["cls"] == cls]
            out[f"acc_{cls}"] = (float(np.mean([r["correct"] for r in sel]))
                                 if sel else float("nan"))
            out[f"n_{cls}"] = len(sel)
        return out


def class_of(cls: str, crossed: int) -> str:
    return {CLS_PRE: "pre", CLS_POST: "post", CLS_CTRL: "ctrl"}[cls]
