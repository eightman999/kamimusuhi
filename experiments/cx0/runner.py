"""CX0 rollout runner — env + OrganSet + architecture arm + interventions.

One ``rollout`` call produces a Trajectory: per-step bundle vectors, actions,
rewards, logits, aux predictions, named population states, and context labels.
Interventions (organ-field lesions/shuffles, cortex-off, hidden resets, memory
erase) are applied inside the loop so every arm sees identical perturbations.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np
import torch

from .env.ctx_world import CtxWorld, EP_LEN, N_ACTIONS, Oracle, RECALL
from .organs.base import OrganSignals
from .organs.modules import OrganSet


@dataclass
class Intervention:
    lesion: str | None = None            # organ field zeroed every step
    shuffle: str | None = None           # organ field replaced by donor seq
    cortex_off: bool = False             # zero core output into heads
    reset_hidden_at: int | None = None   # reset arm state at step t
    reset_organs_at: int | None = None   # reset organ hidden states at t
    erase_memory_at: int | None = None   # wipe slot memory at t
    lesion_pop: str | None = None        # zero one population's output (c2/c3)


@dataclass
class Trajectory:
    bundles: list = field(default_factory=list)      # (T,BUNDLE_DIM) float
    actions: list = field(default_factory=list)
    rewards: list = field(default_factory=list)
    logits: list = field(default_factory=list)
    values: list = field(default_factory=list)
    logps: list = field(default_factory=list)
    aux_pred: list = field(default_factory=list)
    next_sensory: list = field(default_factory=list)
    pops: dict = field(default_factory=dict)          # name -> list of np
    ctx_labels: list = field(default_factory=list)
    oracle_actions: list = field(default_factory=list)
    success: bool = False
    dead: bool = False
    probe_agree: float | None = None                  # CTX-5 only

    def arrays(self):
        return dict(
            bundles=np.stack(self.bundles),
            actions=np.array(self.actions),
            rewards=np.array(self.rewards),
            next_sensory=np.stack(self.next_sensory),
            ctx_labels=np.array(self.ctx_labels),
            oracle_actions=np.array(self.oracle_actions),
        )


def _apply_interventions(bundle: OrganSignals, itv: Intervention,
                         donor_field: np.ndarray | None) -> OrganSignals:
    if itv.lesion:
        bundle = bundle.lesioned(itv.lesion)
    if itv.shuffle and donor_field is not None:
        bundle = bundle.with_field(itv.shuffle, donor_field)
    return bundle


def rollout(task: str, seed: int, organs: OrganSet, arm,
            *, oracle: Oracle | None = None, mix_beta: float = 0.0,
            spec=None,
            itv: Intervention | None = None,
            donor: list[np.ndarray] | None = None,
            record_pops: bool = True,
            sample: bool = False,
            rng: np.random.Generator | None = None) -> Trajectory:
    """Run one episode. If `oracle` given, oracle actions are recorded as
    labels; the executed action is oracle's with prob mix_beta else the
    arm's (argmax or sampled)."""
    itv = itv or Intervention()
    rng = rng or np.random.default_rng(seed * 7919 + 13)
    env = CtxWorld(task, seed=seed, spec=spec)
    organs.reset(seed)
    state = arm.initial_state(1)
    if oracle:
        oracle.reset()
    tr = Trajectory()
    n_agree = n_probe = 0

    obs = env._obs()
    bundle = organs.post_step(obs, env.internal, None)

    for t in range(EP_LEN):
        if env.dead:
            break
        if itv.reset_hidden_at is not None and t == itv.reset_hidden_at:
            state = arm.initial_state(1)
        if itv.reset_organs_at is not None and t == itv.reset_organs_at:
            organs.t0.reset_hidden(); organs.s0.reset_hidden(); organs.h0.reset_hidden()
        if itv.erase_memory_at is not None and t == itv.erase_memory_at:
            organs.memory.erase()

        eff = _apply_interventions(
            bundle, itv,
            donor[t] if donor is not None and t < len(donor) else None)
        x = torch.tensor(eff.concat()[None], dtype=torch.float32)
        arm.set_core_off(itv.cortex_off)
        arm.set_lesioned({itv.lesion_pop} if itv.lesion_pop else frozenset())
        with torch.no_grad():
            logits, value, aux, state, pops = arm(x, state)
        logit_np = logits.squeeze(0).numpy()
        probs = _softmax(logit_np)
        arm_act = int(np.argmax(logit_np)) if not sample else int(
            rng.choice(N_ACTIONS, p=probs))

        act = arm_act
        oa = -1
        if oracle is not None:
            oa = oracle.act(env, organs.memory)
            if rng.random() < mix_beta:
                act = oa
        if task == "ctx5" and env.t in env.spec.probe_times:
            n_probe += 1
            n_agree += int(act - 7 == int(np.argmax(env.probe_options()))) if act >= 7 else 0

        ctx_label = env.context_label()
        organs.pre_step(obs, act)
        info = env.step(act, organs.memory)
        recall = env.last_recall if act == RECALL else None
        obs = info.obs
        bundle = organs.post_step(obs, info.internal, recall)

        tr.bundles.append(eff.concat())
        tr.actions.append(act)
        tr.rewards.append(info.reward)
        tr.logits.append(logit_np)
        tr.values.append(float(value.squeeze(0).item()))
        tr.logps.append(float(np.log(max(probs[act], 1e-9))))
        tr.aux_pred.append(aux.squeeze(0).numpy())
        tr.next_sensory.append(obs.astype(np.float32))
        tr.ctx_labels.append(ctx_label)
        tr.oracle_actions.append(oa)
        if record_pops:
            for k, v in pops.items():
                tr.pops.setdefault(k, []).append(v.squeeze(0).numpy())

    tr.success = env.success
    tr.dead = env.dead
    if task == "ctx5":
        tr.probe_agree = n_agree / max(n_probe, 1)
        tr.success = n_probe > 0 and tr.probe_agree >= 0.8
    return tr


def _softmax(x: np.ndarray) -> np.ndarray:
    e = np.exp(x - x.max())
    return e / e.sum()
