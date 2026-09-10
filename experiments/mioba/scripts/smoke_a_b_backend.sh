#!/usr/bin/env bash
# Smoke A/B: backend smoke on a chosen device/backend.
#   1. import + device recognition        4. artificial-organ run (sparse)
#   2. synthetic FBA initialize            5. checkpoint -> 100 steps -> restore -> same 100 steps identical
#   3. synthetic FBA run                   6. same seed twice -> identical
# On the GPU host: ./smoke_a_b_backend.sh --device cuda:0 --backend torch [--neurons 20000] [--out report.json]
set -u
DEVICE="cpu"; BACKEND="mock"; NEURONS=500; OUT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --device) DEVICE="$2"; shift 2;;
    --backend) BACKEND="$2"; shift 2;;
    --neurons) NEURONS="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    *) echo "unknown arg $1"; exit 2;;
  esac
done
PY="${MIOBA_PYTHON:-/home/ubuntu/mioba-venv/bin/python}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$REPO" || exit 2
"$PY" - "$DEVICE" "$BACKEND" "$NEURONS" "$OUT" <<'EOF'
import json, sys, time
device, backend_name, n_neurons, out = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
from experiments.mioba.development.phenotype import develop
from experiments.mioba.fba.registry import get_backend
from experiments.mioba.fba.runtime_info import collect_runtime_info
from experiments.mioba.genome.schema import (ArtificialOrgan, Attachment,
                                             fba0_genome)
from experiments.mioba.mie.environments import make_drive
DRIVE_CFG = {"env": {"stim_fraction": 0.2, "stim_rate_hz": 200.0}}
report = {"device": device, "backend": backend_name, "n_neurons": n_neurons,
          "steps": {}, "runtime": collect_runtime_info(backend=backend_name)}

def step(name, fn):
    t0 = time.time()
    try:
        info = fn()
        report["steps"][name] = {"ok": True, "wall_s": round(time.time() - t0, 3),
                                 "info": info}
        print(f"PASS {name} {info}")
    except Exception as exc:
        report["steps"][name] = {"ok": False, "wall_s": round(time.time() - t0, 3),
                                 "error": f"{type(exc).__name__}: {exc}"}
        print(f"FAIL {name} {type(exc).__name__}: {exc}")

def device_recognized():
    if device.startswith("cuda"):
        import torch
        assert torch.cuda.is_available(), "torch.cuda.is_available() is False"
        idx = int(device.split(":")[1]) if ":" in device else 0
        p = torch.cuda.get_device_properties(idx)
        return {"name": p.name, "capability": f"{p.major}.{p.minor}",
                "total_mem_gb": round(p.total_memory / 2**30, 2),
                "torch": torch.__version__, "cuda": torch.version.cuda}
    return {"device": device}
step("device_recognized", device_recognized)

def fresh(genome):
    b = get_backend(backend_name, synthetic=True, synthetic_neurons=n_neurons)
    b.initialize(develop(genome), batch_size=2, seed=0, device=device)
    b.set_inputs(make_drive("synthetic-quiet-v0", getattr(b, "n_base", n_neurons), DRIVE_CFG))
    return b

def init_and_run():
    b = fresh(fba0_genome())
    stats = b.run(100)
    s = b.get_state_summary()
    assert stats["simulated_ms"] >= 99.9, stats
    assert s["t_ms"] > 0 and "mean_rate_hz" in s, s
    return {"n": getattr(b, "n", None), "nnz": getattr(b, "nnz", None),
            "dataset": b.dataset_identity(), "mean_rate_hz": s["mean_rate_hz"],
            "vram_bytes": s.get("vram_bytes"), "wall_s": round(stats["wall_s"], 3)}
step("synthetic_init_run", init_and_run)

def organ_run():
    g = fba0_genome()
    g.artificial_organs.append(ArtificialOrgan(organ_id="orgSmoke",
                                               kind="lif_cluster", size=64))
    g.attachments.append(Attachment(attachment_id="attSmoke",
                                    source="fba0:medulla", target="orgSmoke",
                                    direction="bidirectional", weight_scale=1.0))
    b = fresh(g.finalize())
    stats = b.run(100)
    s = b.get_state_summary()
    assert stats["simulated_ms"] >= 99.9
    info = {"n": getattr(b, "n", None), "nnz": getattr(b, "nnz", None),
            "mean_rate_hz": s["mean_rate_hz"], "vram_bytes": s.get("vram_bytes")}
    if hasattr(b, "W") and hasattr(b.W, "layout"):
        import torch
        assert b.W.layout in (torch.sparse_csr, torch.sparse_coo), b.W.layout
        info["layout"] = str(b.W.layout)
    return info
step("artificial_organ_run", organ_run)

def checkpoint_restore():
    b = fresh(fba0_genome())
    b.run(50)
    ck = b.checkpoint()
    b.run(100); a = b.get_state_summary()
    b.run(30)  # move away
    b.restore(ck)
    b.run(100); c = b.get_state_summary()
    assert a["per_batch_spike_counts"] == c["per_batch_spike_counts"], (a, c)
    assert abs(a["t_ms"] - c["t_ms"]) < 1e-6
    return {"spikes": a["per_batch_spike_counts"]}
step("checkpoint_restore_replay", checkpoint_restore)

def same_seed_twice():
    a = fresh(fba0_genome()); a.run(100)
    b = fresh(fba0_genome()); b.run(100)
    sa, sb = a.get_state_summary(), b.get_state_summary()
    assert sa["per_batch_spike_counts"] == sb["per_batch_spike_counts"], (sa, sb)
    assert sum(sa["per_batch_spike_counts"]) > 0, "no spikes; determinism check vacuous"
    return {"spikes": sa["per_batch_spike_counts"]}
step("same_seed_deterministic", same_seed_twice)

ok = all(s["ok"] for s in report["steps"].values())
report["ok"] = ok
if out:
    from pathlib import Path
    Path(out).parent.mkdir(parents=True, exist_ok=True)
    Path(out).write_text(json.dumps(report, indent=2, default=str))
    print(f"wrote {out}")
print(("PASS" if ok else "FAIL") + f" smoke_a_b backend={backend_name} device={device}")
sys.exit(0 if ok else 1)
EOF
