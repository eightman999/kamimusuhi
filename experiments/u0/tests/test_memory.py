"""Slot memory tests for U0."""

import numpy as np

from experiments.u0.memory.slot_memory import SlotMemory


from experiments.u0.env.u0_env import PAYLOAD_DIM


def make(slots=4, dim=PAYLOAD_DIM):
    return SlotMemory(slots, dim)


def payload(v):
    p = np.zeros(PAYLOAD_DIM, dtype=np.float32)
    p[0] = v
    return p


def test_store_and_find():
    m = make()
    m.store(payload(1), func=0, loc=2, potency=1.0)
    assert m.num_occupied == 1
    assert m.find(0, 2) == 0
    assert m.find(0, 3) is None


def test_capacity_eviction_fifo():
    m = make()
    # six distinct (func, loc) pairs into a 4-slot memory
    pairs = [(0, 0), (1, 1), (2, 2), (3, 3), (0, 1), (1, 2)]
    for i, (f, l) in enumerate(pairs):
        m.store(payload(i), func=f, loc=l, potency=0.5)
    assert m.num_occupied == 4
    # first two stores evicted (fifo)
    assert m.find(0, 0) is None
    assert m.find(1, 1) is None
    assert m.find(0, 1) is not None


def test_same_site_refresh():
    m = make()
    s1 = m.store(payload(1), func=0, loc=2, potency=1.0)
    s2 = m.store(payload(2), func=0, loc=2, potency=1.0)
    assert s1 == s2
    assert m.num_occupied == 1


def test_best_for_function_prefers_potency():
    m = make()
    m.store(payload(1), func=0, loc=1, potency=0.45)
    m.store(payload(2), func=0, loc=3, potency=1.0)
    slot, loc = m.best_for_function(0)
    assert loc == 3
    assert m.best_for_function(2) is None


def test_clear_permute_clone():
    m = make()
    m.store(payload(1), func=0, loc=1, potency=1.0)
    m.store(payload(2), func=1, loc=2, potency=0.5)
    st = m.clone_state()
    m.permute(np.array([1, 0, 2, 3]))
    assert m.funcs[0] == 1 and m.funcs[1] == 0
    m.load_state(st)
    assert m.funcs[0] == 0 and m.funcs[1] == 1
    m.clear()
    assert m.num_occupied == 0


def test_recall_block_masked():
    m = make()
    m.store(payload(3), func=0, loc=1, potency=1.0)
    blk = m.recall()
    assert blk.shape == (4, PAYLOAD_DIM)
    assert blk[0, 0] == 3
    assert blk[1:].sum() == 0
