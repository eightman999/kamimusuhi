"""Evaluation plots; matplotlib is imported lazily so headless runs work."""
from pathlib import Path


def _mpl():
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    return plt


def plot_delay_curve(per_delay, path, title="Success rate vs target delay"):
    plt = _mpl()
    delays = sorted(int(d) for d in per_delay)
    success = [per_delay[str(d)]["success"] for d in delays]
    abs_err = [per_delay[str(d)]["mean_abs_error"] if
               per_delay[str(d)]["mean_abs_error"] is not None else float("nan")
               for d in delays]
    fig, ax1 = plt.subplots(figsize=(6, 4))
    ax1.plot(delays, success, "o-", label="success")
    ax1.set_xlabel("target delay (steps)")
    ax1.set_ylabel("success rate")
    ax1.set_ylim(-0.05, 1.05)
    ax2 = ax1.twinx()
    ax2.plot(delays, abs_err, "s--", color="tab:red", label="|error|")
    ax2.set_ylabel("mean |timing error|")
    ax1.set_title(title)
    fig.tight_layout()
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(path, dpi=120)
    plt.close(fig)


def plot_probe_bars(probes, path):
    plt = _mpl()
    names = [k for k in ("elapsed_r2", "remaining_r2", "phase_r2") if k in probes]
    fig, ax = plt.subplots(figsize=(5, 3.5))
    ax.bar(names, [probes[k] for k in names])
    ax.axhline(probes.get("shuffle_max_r2", 0.), color="tab:red", ls="--",
               label="shuffle control")
    ax.set_ylabel("held-out R^2")
    ax.set_ylim(0, 1)
    ax.legend()
    fig.tight_layout()
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(path, dpi=120)
    plt.close(fig)


def plot_interventions(rows, path):
    """``rows``: list of {label, success} dicts."""
    plt = _mpl()
    fig, ax = plt.subplots(figsize=(6, 3.5))
    ax.bar([r["label"] for r in rows], [r["success"] for r in rows])
    ax.set_ylabel("success rate")
    ax.set_ylim(0, 1.05)
    ax.tick_params(axis="x", rotation=30)
    fig.tight_layout()
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(path, dpi=120)
    plt.close(fig)
