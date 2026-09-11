import numpy as np

from ..memory.slot_memory import SlotMemory


def make(m=3, d=8, k=4):
    return SlotMemory(m, d, k)


def payload(k, d=8):
    p = np.zeros(d, dtype=np.float32)
    p[0], p[k % d] = 1.0, 1.0
    return p


def test_store_and_find():
    m = make()
    s = m.store(payload(1), key_idx=1)
    assert m.find_key(1) == s
    assert m.num_occupied == 1


def test_same_key_refreshes_in_place():
    m = make()
    s0 = m.store(payload(1), key_idx=1)
    s1 = m.store(payload(1), key_idx=1)
    assert s0 == s1 and m.num_occupied == 1


def test_fifo_evicts_oldest():
    m = make(m=2)
    m.store(payload(0), key_idx=0)
    m.store(payload(1), key_idx=1)
    m.store(payload(2), key_idx=2)
    assert m.find_key(0) is None
    assert m.find_key(1) is not None and m.find_key(2) is not None


def test_lru_evicts_least_recently_used():
    m = make(m=2)
    m.store(payload(0), key_idx=0, evict="lru")
    m.store(payload(1), key_idx=1, evict="lru")
    m.touch(m.find_key(0))          # key 0 was used most recently
    m.store(payload(2), key_idx=2, evict="lru")
    assert m.find_key(0) is not None
    assert m.find_key(1) is None


def test_recall_returns_payload_block():
    m = make()
    p = payload(2)
    m.store(p, key_idx=2)
    rec = m.recall()
    assert rec.shape == (3, 8)
    assert np.allclose(rec[m.find_key(2)], p)
    assert not rec[~m.occupied].any()


def test_permute_and_clear_and_clone():
    m = make()
    m.store(payload(0), key_idx=0)
    m.store(payload(1), key_idx=1)
    state = m.clone_state()
    m.permute(np.array([1, 2, 0]))
    assert m.num_occupied == 2 and m.find_key(0) is not None
    m.load_state(state)
    assert m.find_key(0) == 0
    m.clear()
    assert m.num_occupied == 0
