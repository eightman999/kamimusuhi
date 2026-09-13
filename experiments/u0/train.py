"""PPO trainer for U0 need-guided memory.

Each PPO iteration rolls out `num_envs` complete episodes (all envs share
the same episode length, so a rollout is exactly T steps and every
recurrent hidden state legitimately restarts from zero). The update
re-runs the stored observation sequences through the policy, which keeps
recurrent models correct without storing hidden states.

Optional teacher bootstrapping (`imitation_iters` > 0) is
*mechanics-only*. Two teacher policies are available:

  * `oracle`  — demonstrates the full recall->move->act recovery
    sequence and good housekeeping; event-present steps are left
    unlabeled (-1) and masked out of the BC loss, because the oracle's
    store-or-ignore choice on an event encodes relevance either way.
  * `store_all` — stores every event indiscriminately, so its labels
    carry no relevance signal at all; labeling is unmasked and the
    student learns the STORE API plus recovery mechanics, while *which*
    events are worth keeping is still left for PPO to discover.

Runs that use a teacher are tagged in run metadata and must not be
conflated with U0-main (pure reward) results.

Budget controls (honored without code changes):
    U0_TIME_BUDGET  seconds; training stops at the next update boundary
    U0_MAX_ITERS    hard cap on PPO iterations
    U0_NUM_ENVS     override env count

Artifacts per run: artifacts/runs/<run_id>/{config.yaml, metrics.jsonl,
best.pt, latest.pt, meta.json}. --resume continues from latest.pt.

Usage:
    python -m experiments.u0.train --config experiments/u0/configs/default.yaml \
        --model gru64 --seed 0 --run-id gru64_s0
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import yaml

from .config import (env_config, load_config, resolve_device,
                     train_config)
from .env.u0_env import (CLS_FUNCTIONAL, EV_NONE, IGNORE, RECALL, STORE,
                         U0Config, VecU0Env, WAIT)
from .evaluate import evaluate_learned
from .models.nets import build_policy
from .policies.baselines import (FIFOPolicy, OraclePolicy,
                                 StoreAllPolicy)

TEACHERS = {"oracle": OraclePolicy, "fifo": FIFOPolicy,
            "store_all": StoreAllPolicy}


def git_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"],
            cwd=Path(__file__).resolve().parents[2],
            stderr=subprocess.DEVNULL).decode().strip()
    except Exception:
        return "unknown"


def collect_rollout(policy, vec: VecU0Env, device, teacher=None,
                    teacher_mix=0.5, mask_events: bool = True,
                    label_teacher: bool = True,
                    teacher_scope: str = "all",
                    keep_student_recall: bool = False,
                    crisis_mask: bool = False,
                    functional_store_only: bool = False):
    """One iteration = one complete episode per env.

    When `teacher` (an OraclePolicy) is given, each step is labeled with
    the action the oracle would take in the student's state — except
    event-present steps, which are left unlabeled (-1) and masked out of
    the BC loss entirely. On an event step the oracle either STOREs or
    IGNOREs depending on relevance, so labeling either way would leak
    the answer ("store this" or "do not store that"); only memory
    *mechanics* and the recall->move->act recovery sequence are taught,
    never what is worth remembering.

    teacher_mix may be a scalar or a per-env array — a selfplay
    fraction of envs at mix 0 gives DAgger-style coverage of the
    student's own state distribution while the rest still demonstrate
    good trajectories for the value function. The scaffold flags
    (crisis_mask / functional_store_only) constrain the *sampled*
    action set during early training; they never label which events
    matter.
    """
    T = vec.envs[0].cfg.episode_len
    B, D = vec.num_envs, vec.obs_dim
    obs_buf = np.zeros((B, T, D), dtype=np.float32)
    act_buf = np.zeros((B, T), dtype=np.int64)
    logp_buf = np.zeros((B, T), dtype=np.float32)
    rew_buf = np.zeros((B, T), dtype=np.float32)
    val_buf = np.zeros((B, T), dtype=np.float32)
    alive_buf = np.zeros((B, T), dtype=bool)   # True while episode lives
    lab_buf = np.full((B, T), -1, dtype=np.int64)  # -1 = no BC target
    ep_stats = []
    teachers = None
    if teacher is not None:
        # one oracle per env: each caches its own episode's need plan
        teachers = [type(teacher)(seed=i) for i in range(B)]
        for te in teachers:
            te.reset()

    obs = vec.reset()
    h = policy.initial_state(B, device)
    policy.eval()
    with torch.no_grad():
        for t in range(T):
            o = torch.as_tensor(obs, device=device)
            logits, v, h = policy(o, h)
            if crisis_mask:
                # scaffold: during an active crisis the idle actions are
                # masked so the student must choose among {STORE, RECALL,
                # MOVE, ACT} — RECALL gets sampled and, when memory holds
                # the item, visibly works. The mask lifts with the
                # scaffold.
                for i, e in enumerate(vec.envs):
                    if not e.done and any(n.active and not n.resolved
                                          for n in e.needs):
                        logits[i, [IGNORE, WAIT]] = -1e9
            if functional_store_only:
                # scaffold: STORE is only available on functional events
                # — junk stores are masked out so the student's memory
                # always holds real sites and the recall pathway can be
                # discovered. Lifts with the scaffold.
                for i, e in enumerate(vec.envs):
                    if (not e.done and e.schedule[e.t].kind != EV_NONE
                            and e.schedule[e.t].cls != CLS_FUNCTIONAL):
                        logits[i, STORE] = -1e9
            dist = torch.distributions.Categorical(logits=logits)
            act = dist.sample()
            if teachers is not None:
                for i, e in enumerate(vec.envs):
                    if e.done:
                        continue
                    la = teachers[i].decide(e)
                    if label_teacher and not (
                            mask_events
                            and e.schedule[e.t].kind != EV_NONE):
                        lab_buf[i, t] = la   # event steps may be unlabeled
                    if teacher_scope == "crisis" and not any(
                            n.active and not n.resolved
                            for n in e.needs):
                        continue          # store decisions stay the
                                          # student's — only crisis
                                          # mechanics are demonstrated
                    if keep_student_recall and act[i] == RECALL:
                        continue  # the student's own retrieval decision
                                  # always executes — it must experience
                                  # "I recalled -> it worked" itself
                    if np.random.random() < teacher_mix[i]:
                        act[i] = la       # execute full teacher action
            logp = dist.log_prob(act)     # logp of the EXECUTED action
            obs_buf[:, t] = obs
            act_buf[:, t] = act.cpu().numpy()
            logp_buf[:, t] = logp.cpu().numpy()
            val_buf[:, t] = v.cpu().numpy()
            alive_buf[:, t] = np.array([not e.done for e in vec.envs])
            obs, rew, done, infos = vec.step(act.cpu().numpy())
            rew_buf[:, t] = rew
            for inf in infos:
                if "ep_stats" in inf:
                    ep_stats.append(inf["ep_stats"])
    policy.train()
    return (obs_buf, act_buf, logp_buf, rew_buf, val_buf,
            ep_stats, lab_buf, alive_buf)


def gae(rew: np.ndarray, val: np.ndarray, alive: np.ndarray,
        gamma: float, lam: float):
    """GAE over single-episode rollouts padded after death.

    `alive[b,t]` marks steps belonging to the episode; post-death padding
    contributes zero advantage and does not propagate backwards."""
    B, T = rew.shape
    adv = np.zeros_like(rew)
    last = np.zeros(B, dtype=np.float32)
    for t in reversed(range(T)):
        live = alive[:, t].astype(np.float32)
        v_next = (val[:, t + 1] * alive[:, t + 1]
                  if t + 1 < T else np.zeros(B, dtype=np.float32))
        delta = (rew[:, t] + gamma * v_next - val[:, t]) * live
        last = delta + gamma * lam * last * live
        adv[:, t] = last
    return adv, adv + val


def _seq_forward(policy, obs_seq: torch.Tensor, device):
    """Re-run a (B, T, D) observation sequence; returns logits (B,T,A) and
    values (B,T). Recurrent policies start from h0=0 (episode aligned)."""
    B, T, _D = obs_seq.shape
    h = policy.initial_state(B, device)
    logits, vals = [], []
    for t in range(T):
        al, v, h = policy(obs_seq[:, t], h)
        logits.append(al)
        vals.append(v)
    return torch.stack(logits, 1), torch.stack(vals, 1)


def ppo_update(policy, opt, bufs, tc, device):
    obs_buf, act_buf, logp_buf, rew_buf, val_buf, alive_buf = bufs
    adv, ret = gae(rew_buf, val_buf, alive_buf, tc["gamma"], tc["lam"])
    B, T, D = obs_buf.shape

    obs_t = torch.as_tensor(obs_buf, device=device)
    act_t = torch.as_tensor(act_buf, device=device)
    old_logp_t = torch.as_tensor(logp_buf, device=device)
    adv_t = torch.as_tensor(adv, device=device)
    ret_t = torch.as_tensor(ret, device=device)
    mask_t = torch.as_tensor(alive_buf, dtype=torch.float32, device=device)
    live = adv_t[mask_t.bool()]
    adv_t = torch.where(
        mask_t.bool(),
        (adv_t - live.mean()) / (live.std() + 1e-8),
        torch.zeros_like(adv_t))

    clip, vfc, ent = tc["clip"], tc["vf_coef"], tc["ent_coef"]
    mb = tc.get("minibatch_episodes", B)
    pl = vl = el = 0.0
    nup = 0
    for _ in range(tc["epochs"]):
        perm = torch.randperm(B, device=device)
        for lo in range(0, B, mb):
            idx = perm[lo:lo + mb]
            m = mask_t[idx]
            al_s, v_s = _seq_forward(policy, obs_t[idx], device)
            dist = torch.distributions.Categorical(logits=al_s)
            logp = dist.log_prob(act_t[idx])
            ratio = torch.exp(logp - old_logp_t[idx])
            a = adv_t[idx]
            pi_el = torch.max(
                -a * ratio,
                -a * torch.clamp(ratio, 1 - clip, 1 + clip))
            pi_loss = (pi_el * m).sum() / m.sum().clamp(min=1)
            v_el = 0.5 * (v_s - ret_t[idx]).pow(2)
            v_loss = (v_el * m).sum() / m.sum().clamp(min=1)
            ent_b = (dist.entropy() * m).sum() / m.sum().clamp(min=1)
            loss = pi_loss + vfc * v_loss - ent * ent_b
            opt.zero_grad()
            loss.backward()
            nn.utils.clip_grad_norm_(policy.parameters(),
                                     tc["max_grad_norm"])
            opt.step()
            pl += pi_loss.item()
            vl += v_loss.item()
            el += ent_b.item()
            nup += 1
    return {"pi_loss": pl / nup, "v_loss": vl / nup, "entropy": el / nup}


def bc_update(policy, opt, obs_buf, lab_buf, alive_buf, tc, device):
    """Mechanics-only behavioral cloning on teacher-labeled student steps.

    lab_buf entries are -1 on masked steps (oracle-teacher event steps,
    where either label would leak relevance) and on padded steps; they
    are excluded from the loss so the teacher never reveals which events
    are worth remembering.
    """
    B, T, D = obs_buf.shape
    obs_t = torch.as_tensor(obs_buf, device=device)
    lab_t = torch.as_tensor(lab_buf, device=device)
    mask_t = torch.as_tensor(alive_buf, dtype=torch.float32,
                             device=device)
    mb = tc.get("minibatch_episodes", B)
    total = 0.0
    nup = 0
    for _ in range(tc["epochs"]):
        perm = torch.randperm(B, device=device)
        for lo in range(0, B, mb):
            idx = perm[lo:lo + mb]
            al_s, _v = _seq_forward(policy, obs_t[idx], device)
            al = al_s.reshape(-1, al_s.shape[-1])
            lab = lab_t[idx].reshape(-1)
            valid = (mask_t[idx].reshape(-1) > 0) & (lab >= 0)
            freq = torch.bincount(lab[valid],
                                  minlength=al.shape[-1]).float()
            w = (1.0 / freq.clamp(min=1))
            w = w * (al.shape[-1] / w.sum())   # class-balanced act CE
            el = torch.nn.functional.cross_entropy(
                al, lab.clamp(min=0), weight=w, reduction="none")
            loss = (el * valid.float()).sum() / valid.sum().clamp(min=1)
            opt.zero_grad()
            loss.backward()
            nn.utils.clip_grad_norm_(policy.parameters(),
                                     tc["max_grad_norm"])
            opt.step()
            total += loss.item()
            nup += 1
    return {"bc_loss": total / max(1, nup)}


def save_ckpt(policy, path: Path, **meta) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    torch.save({"model": policy.state_dict(), **meta}, path)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--model", default="gru64",
                    choices=["mlp", "gru64", "gru128"])
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--artifacts", default="experiments/u0/artifacts")
    ap.add_argument("--run-id", default=None)
    ap.add_argument("--device", default="cpu",
                    choices=["cpu", "mps", "auto"])
    ap.add_argument("--imitation-iters", type=int, default=None,
                    help="override train.imitation_iters (mechanics-only "
                         "teacher bootstrap)")
    ap.add_argument("--teacher", default=None,
                    choices=list(TEACHERS),
                    help="override train.teacher (oracle masks event "
                         "steps; fifo/store_all are relevance-blind so "
                         "their labels are used verbatim)")
    ap.add_argument("--scaffold", action="store_true",
                    help="apply the scaffold curriculum arm: crisis "
                         "action-masking + functional-only stores early "
                         "in training + delay annealing. Disclosed in "
                         "the run config — the gate itself is still "
                         "reward-learned")
    ap.add_argument("--resume", action="store_true")
    args = ap.parse_args()

    torch.set_num_threads(min(8, os.cpu_count() or 1))
    cfg = load_config(args.config)
    ecfg = env_config(cfg, seed=args.seed)
    tc = train_config(cfg)
    if args.imitation_iters is not None:
        tc["imitation_iters"] = args.imitation_iters
    if args.teacher is not None:
        tc["teacher"] = args.teacher
    if args.scaffold:
        tc.update(ent_coef=max(tc.get("ent_coef", 0.01), 0.008),
                  crisis_mask_iters=tc.get("crisis_mask_iters") or 200,
                  functional_store_iters=tc.get("functional_store_iters")
                  or 200,
                  delay_curriculum_start=tc.get("delay_curriculum_start")
                  or 8,
                  delay_curriculum_iters=tc.get("delay_curriculum_iters")
                  or 300)
    iters = int(os.environ.get("U0_MAX_ITERS", tc["iters"]))
    num_envs = int(os.environ.get("U0_NUM_ENVS", tc["num_envs"]))
    budget = float(os.environ.get("U0_TIME_BUDGET", "0"))

    run_id = args.run_id or f"{args.model}_s{args.seed}"
    run_dir = Path(args.artifacts) / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)

    torch.manual_seed(args.seed)
    np.random.seed(args.seed)
    device = torch.device(resolve_device(args.device))
    vec = VecU0Env(ecfg, num_envs, seed=args.seed * 977 + 13)
    policy = build_policy(args.model, vec.obs_dim).to(device)
    opt = torch.optim.Adam(policy.parameters(), lr=tc["lr"])

    start_iter = 0
    best_err = float("inf")
    last_pt = run_dir / "latest.pt"
    if args.resume and last_pt.exists():
        state = torch.load(last_pt, map_location="cpu", weights_only=True)
        policy.load_state_dict(state["model"])
        start_iter = int(state.get("iter", 0))
        metrics_path = run_dir / "metrics.jsonl"
        if metrics_path.exists():
            # drop rows recorded after the checkpoint being resumed:
            # they belong to an orphaned suffix of the trajectory the
            # restored weights never saw
            lines = [l for l in metrics_path.read_text().splitlines()
                     if l.strip()]
            kept = [l for l in lines
                    if int(json.loads(l)["iter"]) <= start_iter]
            if len(kept) != len(lines):
                metrics_path.write_text(
                    "\n".join(kept) + ("\n" if kept else ""))
        best_pt = run_dir / "best.pt"
        if best_pt.exists():
            st = torch.load(best_pt, map_location="cpu",
                            weights_only=True)
            # selection metric is crisis_error_auc; tolerate older
            # checkpoints that only recorded val_error_full
            best_err = float(st.get("val_crisis_error_auc",
                                    st.get("val_error_full",
                                           float("inf"))))
        print(f"[{run_id}] resume at iter {start_iter} "
              f"(best_err={best_err:.4f})", flush=True)

    teacher_name = tc.get("teacher", "oracle")
    teacher = TEACHERS[teacher_name](seed=args.seed) \
        if tc.get("imitation_iters", 0) else None
    # an oracle teacher's event-step choices encode relevance, so those
    # steps are masked out of BC; a relevance-blind teacher (store_all /
    # fifo) can label everything — it teaches the STORE API without
    # revealing what is worth keeping
    mask_events = teacher_name == "oracle"

    (run_dir / "config.yaml").write_text(yaml.safe_dump(
        {"env": {k: (list(v) if isinstance(v, tuple) else v)
                 for k, v in ecfg.__dict__.items()},
         "train": tc, "model": args.model, "seed": args.seed,
         "run_id": run_id, "git_commit": git_commit(),
         "torch_version": str(torch.__version__), "device": str(device),
         "teacher": teacher_name if tc.get("imitation_iters")
         else None}, sort_keys=False))
    metrics_f = open(run_dir / "metrics.jsonl", "a")

    it = start_iter   # keeps it bound if the range below is empty
    # optional delay curriculum: begin with needs firing soon after the
    # event window (tight store->outcome credit) and grow toward the
    # configured delay. Only *newly reset* episodes see the current
    # delay, so the change takes effect as episodes roll over.
    delay_start = tc.get("delay_curriculum_start")
    delay_anneal = tc.get("delay_curriculum_iters", 0)
    delay_target = (ecfg.delay_min, ecfg.delay_max)
    if delay_start is not None:
        ecfg.delay_min = ecfg.delay_max = int(delay_start)
    # optional memory-prefill scaffold: early in training the env injects
    # the needed site at onset (teaches the recall chain and lets the
    # critic learn "needed func held -> resolution"); annealed to 0 so
    # the student's own stores must produce that state. The gate itself
    # is still learned from reward — nothing labels which events matter.
    prefill_start = tc.get("prefill_start")
    prefill_iters = tc.get("prefill_iters", 0)
    prefill_hold = tc.get("prefill_hold", 0)
    auto_recall_iters = tc.get("auto_recall_iters", 0)
    if prefill_start is not None:
        ecfg.prefill_need_prob = float(prefill_start)
    if auto_recall_iters:
        ecfg.auto_recall_prob = 1.0
    crisis_mask_iters = tc.get("crisis_mask_iters", 0)
    store_cost_target = ecfg.store_cost
    if crisis_mask_iters:
        # while the crisis action-mask is on, memory ops are free so the
        # STORE action is not suppressed before the gate can form
        ecfg.store_cost = 0.0
    # checkpoint selection always evaluates the *target* task, not the
    # current curriculum stage
    if (delay_start is not None or prefill_start is not None
            or auto_recall_iters):
        ecfg_eval = U0Config(**ecfg.__dict__)
        ecfg_eval.delay_min, ecfg_eval.delay_max = delay_target
        ecfg_eval.prefill_need_prob = 0.0
        ecfg_eval.auto_recall_prob = 0.0
    else:
        ecfg_eval = ecfg
    t0 = time.time()
    for it in range(start_iter + 1, iters + 1):
        if delay_start is not None and delay_anneal > 0:
            frac = min(1.0, it / delay_anneal)
            ecfg.delay_min = int(round(
                delay_start + frac * (delay_target[0] - delay_start)))
            ecfg.delay_max = int(round(
                delay_start + frac * (delay_target[1] - delay_start)))
        if auto_recall_iters:
            ecfg.auto_recall_prob = max(0.0, 1.0 - it / auto_recall_iters)
        if crisis_mask_iters:
            ecfg.store_cost = 0.0 if it <= crisis_mask_iters \
                else store_cost_target
        if prefill_start is not None:
            if it <= prefill_hold:
                ecfg.prefill_need_prob = float(prefill_start)
            elif prefill_iters > 0:
                ecfg.prefill_need_prob = float(prefill_start) * max(
                    0.0, 1.0 - (it - prefill_hold) / prefill_iters)
        imit = tc["imitation_iters"]
        fade = tc.get("teacher_fade_iters", 0)
        floor = tc.get("teacher_mix_floor", 0.0)
        in_imit = teacher is not None and it <= imit
        if teacher is None:
            mix = 0.0
        elif in_imit:
            mix = tc["teacher_mix"]
        elif it <= imit + fade:
            mix = floor + (tc["teacher_mix"] - floor) \
                * (1.0 - (it - imit) / max(fade, 1))
        else:
            mix = floor     # assisted exploration never fully switches
                            # off when floor > 0: the value function keeps
                            # seeing resolved crises so the store->outcome
                            # gradient stays alive
        if teacher is None or (not in_imit and mix <= 0):
            mix_vec = None
        else:
            # per-env mix: the selfplay fraction runs purely on the
            # student's own actions (its labels still come from the
            # teacher, so its own states are covered — DAgger), while
            # the rest get teacher-executed demonstrations
            mix_vec = np.full(tc["num_envs"], float(mix))
            n_self = int(round(tc.get("teacher_selfplay_frac", 0.0)
                               * tc["num_envs"]))
            mix_vec[:n_self] = 0.0
        bufs = collect_rollout(
            policy, vec, device,
            teacher=teacher if mix_vec is not None else None,
            teacher_mix=(mix_vec if mix_vec is not None else 0.0),
            mask_events=mask_events,
            label_teacher=in_imit,
            teacher_scope=tc.get("teacher_scope", "all"),
            keep_student_recall=tc.get(
                "teacher_keep_student_recall", False),
            crisis_mask=bool(tc.get("crisis_mask_iters", 0)
                             and it <= tc["crisis_mask_iters"]),
            functional_store_only=bool(
                tc.get("functional_store_iters", 0)
                and it <= tc["functional_store_iters"]))
        if in_imit:
            losses = bc_update(policy, opt, bufs[0], bufs[6], bufs[7],
                               tc, device)
        else:
            losses = ppo_update(policy, opt,
                                (*bufs[:5], bufs[7]), tc, device)
        stats = bufs[5]
        agg = {k: float(np.nanmean([s[k] for s in stats]))
               for k in stats[0]
               if isinstance(stats[0][k], (int, float))} if stats else {}
        rec = {"iter": it, "elapsed": round(time.time() - t0, 1),
               "mean_return": float(bufs[3].sum(1).mean()),
               **{f"ep_{k}": v for k, v in agg.items()}, **losses}
        if it % tc["eval_every"] == 0 or it == iters:
            val = evaluate_learned(policy, ecfg_eval, str(device),
                                   episodes=tc["eval_episodes"],
                                   seed=tc["val_seed"])
            rec["val_error_full"] = val["error_full"]
            rec["val_crisis_error_auc"] = val["crisis_error_auc"]
            rec["val_need_resolution"] = val["need_resolution"]
            rec["val_important_retention"] = val["important_retention"]
            rec["val_store_precision"] = val["store_precision"]
            # checkpoint selection tracks the primary loss: mean
            # crisis-window error integral (lower = faster recovery)
            if val["crisis_error_auc"] < best_err:
                best_err = val["crisis_error_auc"]
                save_ckpt(policy, run_dir / "best.pt",
                          model_name=args.model, iter=it,
                          val_crisis_error_auc=best_err,
                          val_error_full=val["error_full"])
        if it % tc.get("checkpoint_every", 10) == 0 or it == iters:
            save_ckpt(policy, last_pt, model_name=args.model, iter=it)
        metrics_f.write(json.dumps(rec) + "\n")
        metrics_f.flush()
        print(f"[{run_id}] it {it}/{iters} ret {rec['mean_return']:.3f} "
              f"err {agg.get('error_full', float('nan')):.4f} "
              f"ret_res {agg.get('need_resolution', 0):.2f} "
              f"val {rec.get('val_error_full', float('nan')):.4f}",
              flush=True)
        if budget and time.time() - t0 > budget:
            break

    save_ckpt(policy, last_pt, model_name=args.model, iter=it)
    (run_dir / "meta.json").write_text(json.dumps(
        {"iters": it, "best_val_crisis_error_auc": best_err,
         "elapsed_sec": round(time.time() - t0, 1),
         "device": str(device),
         "torch_num_threads": torch.get_num_threads(),
         "git_commit": git_commit()}, indent=2))
    print(f"[{run_id}] done iters={it} best_val_auc={best_err:.4f}")


if __name__ == "__main__":
    main()
