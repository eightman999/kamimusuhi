"""CX0 tests — world kernel, memory, organ bundle, arms, runner."""

import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).parents[3]))

from experiments.cx0.env.ctx_world import (EP_LEN, N_ETYPES, N_LOCS, CtxWorld,
                                           Oracle, TASKS)
from experiments.cx0.memory.slot_memory import SlotMemory
from experiments.cx0.organs.base import BUNDLE_DIM, DIMS, OrganSignals


# ----------------------------------------------------------------------
# world kernel
# ----------------------------------------------------------------------
class TestWorld:
    @pytest.mark.parametrize("task", list(TASKS))
    def test_oracle_solves(self, task):
        succ = died = 0
        for ep in range(15):
            w = CtxWorld(task, seed=ep)
            m = SlotMemory(3, 8, 6)
            o = Oracle(task)
            o.reset()
            for t in range(EP_LEN):
                if w.dead:
                    break
                w.step(o.act(w, m), m)
            succ += int(w.success)
            died += int(w.dead)
        if task == "ctx5":
            pytest.skip("ctx5 success is probe agreement, scored by runner")
        assert succ >= 13, f"oracle success {succ}/15"
        assert died == 0

    def test_deterministic(self):
        outs = []
        for _ in range(2):
            w = CtxWorld("ctx1", seed=7)
            m = SlotMemory(3, 8, 6)
            seq = []
            for t in range(EP_LEN):
                if w.dead:
                    break
                info = w.step((t * 3 + 1) % 11, m)
                seq.append(tuple(np.round(info.obs, 6)))
            outs.append(seq)
        assert outs[0] == outs[1]

    def test_state_snapshot_roundtrip(self):
        w = CtxWorld("ctx1", seed=3)
        m = SlotMemory(3, 8, 6)
        for _ in range(30):
            w.step(1, m)
        st = w.get_state()
        w2 = CtxWorld("ctx1", seed=99)
        w2.set_state(st)
        assert w2.pos == w.pos and np.allclose(w2.internal, w.internal)
        # continuing both worlds identically must match
        for _ in range(10):
            a = w.step(1, m)
            b = w2.step(1, m)
            assert np.allclose(a.obs, b.obs)

    def test_no_crisis_refire(self):
        w = CtxWorld("ctx1", seed=5)
        m = SlotMemory(3, 8, 6)
        o = Oracle("ctx1")
        o.reset()
        fired = 0
        for t in range(EP_LEN):
            if w.dead:
                break
            prev_need = w.crisis_need
            w.step(o.act(w, m), m)
            if w.crisis_need >= 0 and prev_need < 0:
                fired += 1
        assert fired <= 1

    @pytest.mark.parametrize("task", ["ctx1", "ctx2", "ctx4"])
    def test_perception_matches_obs(self, task):
        # The event dims the agent saw in obs_t must equal the event
        # step() uses/stores at t — perception is one roll per step.
        w = CtxWorld(task, seed=3)
        m = SlotMemory(3, 8, 6)
        prev_obs_et = None
        for t in range(EP_LEN):
            if w.dead:
                break
            info = w.step(1, m)   # FWD
            if prev_obs_et is not None:
                assert info.event_etype == prev_obs_et, (
                    f"{task} t={t}: step perceived {info.event_etype} "
                    f"but obs showed {prev_obs_et}")
            prev_obs_et = (int(round(info.obs[9] * N_ETYPES))
                           if info.obs[8] > 0.5 else 0)

    def test_perceived_event_cached_within_step(self):
        # Repeat calls in the same step return the same event and burn
        # no RNG — the phantom TTL is step-scoped, not call-scoped.
        w = CtxWorld("ctx2", seed=3)
        m = SlotMemory(3, 8, 6)
        for t in range(40):
            if w.dead:
                break
            w.step(1, m)
            a = w._perceived_event()
            st = w.rng.bit_generator.state
            assert w._perceived_event() == a
            assert w.rng.bit_generator.state == st

    def test_store_noop_without_event(self):
        # STORE with nothing perceived must not occupy a slot.
        w = CtxWorld("ctx1", seed=3)
        m = SlotMemory(3, 8, 6)
        # park off-site after the announce window so no event is visible
        w.t = TASKS["ctx1"].announce_window[1] + 1
        w.pos = int(np.flatnonzero(w.sites == 0)[0])
        w.step(5, m)   # STORE
        assert m.occupied.sum() == 0


# ----------------------------------------------------------------------
# memory
# ----------------------------------------------------------------------
class TestMemory:
    def test_store_recall(self):
        m = SlotMemory(3, 8, 6)
        k = np.zeros(6); k[1] = 1.0
        m.store(np.arange(8, dtype=np.float32), k)
        pl, sc, s = m.recall(k)
        assert sc > 0.99 and pl[1] == 1.0

    def test_refresh_same_key(self):
        m = SlotMemory(3, 8, 6)
        k = np.zeros(6); k[2] = 1.0
        m.store(np.ones(8, dtype=np.float32), k)
        m.store(np.zeros(8, dtype=np.float32), k)
        assert m.occupied.sum() == 1

    def test_eviction(self):
        m = SlotMemory(3, 8, 6)
        for i in range(4):
            k = np.zeros(6); k[i] = 1.0
            m.store(np.full(8, float(i), dtype=np.float32), k)
        assert m.occupied.sum() == 3

    def test_shuffle_preserves_set(self):
        m = SlotMemory(3, 8, 6)
        for i in range(3):
            k = np.zeros(6); k[i] = 1.0
            m.store(np.full(8, float(i + 1), dtype=np.float32), k)
        before = np.sort(m.payloads.sum(1))
        m.shuffle_payloads(np.random.default_rng(0))
        assert np.allclose(before, np.sort(m.payloads.sum(1)))

    def test_content_readout_perm_noop(self):
        # R0-C3 analog: permuting slot order does not change recall result
        m = SlotMemory(3, 8, 6)
        ks = []
        for i in range(3):
            k = np.zeros(6); k[i] = 1.0
            ks.append(k)
            m.store(np.full(8, float(i + 1), dtype=np.float32), k)
        pl1, s1, _ = m.recall(ks[2])
        perm = np.random.default_rng(0).permutation(3)
        m.payloads = m.payloads[perm]
        m.key_vecs = m.key_vecs[perm]
        pl2, s2, _ = m.recall(ks[2])
        assert np.allclose(pl1, pl2) and s1 == s2


# ----------------------------------------------------------------------
# bundle
# ----------------------------------------------------------------------
class TestBundle:
    def test_dims(self):
        b = OrganSignals.zeros()
        v = b.concat()
        assert v.shape == (BUNDLE_DIM,)
        off = 0
        for f, d in DIMS.items():
            assert getattr(b, f).shape == (d,)
            off += d
        assert off == BUNDLE_DIM

    def test_lesion_and_field(self):
        b = OrganSignals.zeros()
        b = b.with_field("h0", np.ones(DIMS["h0"], dtype=np.float32))
        assert b.concat()[DIMS["sensory"]:DIMS["sensory"] + DIMS["h0"]].sum() \
            == DIMS["h0"]
        l = b.lesioned("h0")
        assert l.h0.sum() == 0 and l.sensory.sum() == 0


# ----------------------------------------------------------------------
# arms
# ----------------------------------------------------------------------
class TestArms:
    def test_param_parity(self):
        import torch
        from experiments.cx0.models.arms import C1GRU, C2Layered, C3Mantle, count_params
        p = {n: count_params(c()) for n, c in
             [("c1", C1GRU), ("c2", C2Layered), ("c3", C3Mantle)]}
        lo, hi = min(p.values()), max(p.values())
        assert lo / hi > 0.90, p

    def test_forward_shapes(self):
        import torch
        from experiments.cx0.models.arms import ARMS
        for name, cls in ARMS.items():
            arm = cls()
            st = arm.initial_state(2)
            lg, v, aux, st, pops = arm(torch.zeros(2, BUNDLE_DIM), st)
            assert lg.shape == (2, 11) and v.shape == (2,)
            assert aux.shape == (2, 16)

    def test_cortex_off_and_lesion(self):
        import torch
        from experiments.cx0.models.arms import C3Mantle
        arm = C3Mantle()
        st = arm.initial_state(1)
        x = torch.randn(1, BUNDLE_DIM)
        lg1, *_ = arm(x, st)
        arm.set_core_off(True)
        st = arm.initial_state(1)
        lg2, *_ = arm(x, st)
        arm.set_core_off(False)
        arm.set_lesioned({"context"})
        st = arm.initial_state(1)
        lg3, _, _, st, pops = arm(x, st)
        assert not torch.allclose(lg1, lg2)
        assert pops["context"].abs().sum() == 0
        assert st["context"].abs().sum() == 0


# ----------------------------------------------------------------------
# runner
# ----------------------------------------------------------------------
class TestRunner:
    def test_rollout_runs(self, tmp_path):
        import torch
        from experiments.cx0.models.arms import C0Flat
        from experiments.cx0.organs.pretrain import build_organ_set
        from experiments.cx0.runner import rollout
        org = build_organ_set(tmp_path / "organs")
        arm = C0Flat()
        tr = rollout("ctx1", seed=0, organs=org, arm=arm)
        assert len(tr.actions) > 0
        assert tr.bundles[0].shape == (BUNDLE_DIM,)

    def test_lesion_changes_input(self, tmp_path):
        import numpy as np
        from experiments.cx0.runner import Intervention, _apply_interventions
        from experiments.cx0.organs.base import OrganSignals
        b = OrganSignals.zeros()
        b = b.with_field("t0", np.ones(18, dtype=np.float32))
        out = _apply_interventions(b, Intervention(lesion="t0"), None)
        assert out.t0.sum() == 0
        out = _apply_interventions(b, Intervention(shuffle="t0"),
                                   np.full(18, 0.5, dtype=np.float32))
        assert np.allclose(out.t0, 0.5)
