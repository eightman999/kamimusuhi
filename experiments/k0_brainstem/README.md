# K0-E Parallel Artificial Brainstem

16-dimensional float sensor streams → recurrent policy → six actions → evaluation-only language organ. J72 is not the Core and is never called by the trainer. This experiment is isolated from Kamimusuhi's authoritative continuity state.

## Run

Create an isolated Python environment; install `requirements.txt`. Linux CUDA wheels must support **sm_60 (P100)** as well as sm_86 (3060); the verified runtime is torch 2.13.0+cu126. No DDP. Inventory GPUs before running; launcher selects physical UUIDs rather than assuming indexes.

```bash
python -m experiments.k0_brainstem.train.trainer --config experiments/k0_brainstem/configs/smoke.yaml --artifacts experiments/k0_brainstem/artifacts --run-id smoke --device cuda:0
python -m experiments.k0_brainstem.monitor.server --artifacts experiments/k0_brainstem/artifacts --port 8097
```

Monitor binds 127.0.0.1; use an SSH tunnel. On Mac, install `requirements-monitor.txt` into `.venv-k0-monitor`, then double-click `run_k0_monitor.command`. HTTP and WebSocket use the same tunnel. GUI/server termination cannot terminate detached trainers. PAUSE/RESUME are cooperative at update boundaries; STOP saves state and requires a GUI confirmation.

Only after actual smoke gates (GPU training, files, GUI, pause/resume, reconnect, telemetry) pass, write `artifacts/smoke_gate.json` with `passed: true` and evidence, then:

```bash
python -m experiments.k0_brainstem.train.launcher --artifacts experiments/k0_brainstem/artifacts
```

The 24-run queue uses one process per physical GPU and skips completed runs on restart. Failed/stopped runs require explicit diagnosis and resume; they are not automatically repeated. A live-process collision aborts. `--resume` restores optimizer, recurrent episode boundary, all RNGs including the environment generator. Controls do not migrate the run across GPUs.

## Fixed initial protocol

Six architectures × four seeds, 8192 environments × 128-step complete episodes. Each update collects one full batch; 16 imitation updates (class-balanced loss, 32× decision retention weight, 4× novelty weight) then 8 PPO updates (two epochs, 256-environment sequence minibatches). 25,165,824 environment transitions/run, excluding validation. No truncated timestep shuffling. Selection uses a fixed separate validation seed 700001 and balanced task/retention/habituation/novelty/required-call score minus call cost. Save final, warmup and best checkpoints separately.

Sparse seeds 0/1 use 10% connectivity, seeds 2/3 use 25%; this is a **two-seed density subgroup** inside the four-run architecture budget, not four replicates per density. Interpret pooled sparse results with this limitation. No extra architecture search is hidden in the 24 runs.

A two-size GRU-64 pilot compares 8192 and 16384. The initial protocol retains 8192 if warmed throughput gain is less than 10%, preserving a common per-update dataset size across both GPUs.

All models see only numerical observations. Oracle labels and scenario IDs stay outside Core input. Delayed-cue counterfactual tests regenerate episodes with opposite hidden cues and assert identical observations after t=0. Habituation reports both repeated-IGNORE accuracy and the joint initial-ORIENT → repeated-IGNORE → renewed-ORIENT score; constant IGNORE cannot pass the latter.

## Evaluation and interpretation

```bash
python -m experiments.k0_brainstem.eval.evaluator --checkpoint PATH/best.pt --device cpu --episodes 512
python -m experiments.k0_brainstem.eval.live_episode --checkpoint PATH/best.pt --endpoint "$J72_ENDPOINT" --episodes 50
python -m experiments.k0_brainstem.eval.report --artifacts experiments/k0_brainstem/artifacts
```

All architectures use CPU-generated, held-out seed 900001 to avoid GPU-specific random corpora. Seven OOD perturbations include noise, dropout, inversion, longer delays, event combinations, resource drops, and sensor faults. Labels retain physical event truth under corrupted sensors. Report macro task success, conditional decision accuracy and actual decision counts, not only idle-dominated global accuracy. State-reset ablation checks whether a recurrent checkpoint uses memory. Gate-bias sweeps (-2,-1,0,1,2) are descriptive evaluation curves, not additional trained policies; do not select on held-out test data.

Warmup vs PPO is paired on the same held-out corpus. The result combines further learning and reward/call cost; it does not isolate the causal effect of cost without a separate no-cost training control. Four seeds are the replication units. Do not inflate statistical significance by treating thousands of episodes as independent training runs. Report effect sizes and all seed values; success conditions may remain unmet.

Real language evaluation serializes event/state only after INVOKE_LANGUAGE. Timeout/HTTP/schema errors become `LANGUAGE_BACKEND_UNAVAILABLE`. Language event JSONL distinguishes virtual and real calls. Model latency is CPU batch-1, four PyTorch threads, warmed mean over 500 calls; this is not physical Edge latency.

Run data live in `artifacts/runs/`; full weights, SQLite, transient logs and private network addresses are not committed. Portable aggregate tables, plots and report can be explicitly included as initial experiment evidence. No automatic service restoration/deployment or further phase is implied.

## Checks

```bash
python -m pytest experiments/k0_brainstem/tests/test_core.py experiments/k0_brainstem/tests/test_training.py
QT_QPA_PLATFORM=offscreen .venv-k0-monitor/bin/python -m pytest experiments/k0_brainstem/tests/test_monitor.py
```
