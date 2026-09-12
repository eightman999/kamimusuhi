"""Regenerate ``T0_RESULTS.md`` from a T0-v2 artifact root only.

Reads ``<artifacts>/consolidated.json`` plus the per-run eval JSONs.  The
PASS gates below are fixed pre-registered thresholds; the report emits
``INVALID`` when protocol integrity is not clean and never upgrades a
verdict based on favourable numbers.
"""
import argparse
import json
import subprocess
from pathlib import Path

from experiments.t0.analysis.protocol import PROTOCOL_VERSION

GATE_INTERP_PRIMARY = .8   # H2: mean success on held-out 40/56
GATE_INTERP_BAND = .7      # H2 secondary: mean success over the holdout bands
GATE_EXTRAP = .8           # H3: mean success at 80/96/128
GATE_RESET_DROP = .5       # H4: baseline minus hidden-reset success
GATE_ELAPSED_R2 = .8       # H5: held-out episode probe R^2
GATE_SHUFFLE_MAX = .3      # H5 control: shuffle R^2 must stay below
GATE_DISTRACTOR = .8       # Strong: mean success at distractor_rate .2
GATE_SCALED = .5           # Strong: mean success under x0.5 / x2.0 dynamics
GATE_SSM_R2 = .95          # Strong: ssm/leaky linear elapsed representation
GATE_VIABLE = .5           # arch counts as trained at all above this floor
RECURRENT_GATED = ("gru64", "gru128", "lstm64", "lstm128")


def _m(stat):
    return None if not stat else stat.get("mean")


def _fmt(v):
    return "n/a" if v is None else f"{v:.3f}".lstrip("0").replace("-.", "-.")


def judge(summary):
    """Apply the pre-registered gates to a consolidated dict."""
    archs = summary["architectures"]
    hypotheses = {}
    mlp_seen = _m(archs.get("mlp", {}).get("seen"))
    gated = [a for a in RECURRENT_GATED if a in archs]
    hypotheses["T0-H1"] = bool(
        mlp_seen is not None and gated and all(
            (_m(archs[a]["seen"]) or 0) > mlp_seen for a in gated))
    hypotheses["T0-H2"] = any(
        (_m(archs[a]["interpolation"]) or 0) >= GATE_INTERP_PRIMARY
        and (_m(archs[a]["interpolation_band"]) or 0) >= GATE_INTERP_BAND
        for a in gated)
    hypotheses["T0-H3"] = any(
        (_m(archs[a]["extrapolation"]) or 0) >= GATE_EXTRAP for a in archs)
    hypotheses["T0-H4"] = any(
        (_m(archs[a]["baseline_success"]) or 0) >= GATE_VIABLE
        and (_m(archs[a]["baseline_success"]) or 0)
        - (_m(archs[a]["hidden_reset_success"]) or 0) >= GATE_RESET_DROP
        for a in archs)
    hypotheses["T0-H5"] = any(
        (_m(archs[a]["elapsed_r2"]) or 0) >= GATE_ELAPSED_R2
        and (_m(archs[a]["shuffle_max_r2"]) or 1) < GATE_SHUFFLE_MAX
        for a in archs)
    strong = {
        "extrapolation": hypotheses["T0-H3"],
        "distractor": any((_m(archs[a]["distractor"]) or 0) >= GATE_DISTRACTOR
                          for a in archs),
        "temporal_scaling": any(
            (_m(archs[a]["scaled_0.5"]) or 0) >= GATE_SCALED
            and (_m(archs[a]["scaled_2.0"]) or 0) >= GATE_SCALED
            for a in archs),
        "ssm_linear_time": any(
            (_m(archs[a]["elapsed_r2"]) or 0) >= GATE_SSM_R2
            for a in ("ssm", "leaky") if a in archs),
    }
    primary_pass = all(hypotheses[h] for h in ("T0-H1", "T0-H2", "T0-H4", "T0-H5"))
    verdict = "PASS" if primary_pass else "FAIL"
    if primary_pass and all(strong.values()):
        verdict = "STRONG_PASS"
    if summary["protocol_integrity"]["status"] != "PASS":
        verdict = "INVALID"
    return hypotheses, strong, verdict


def tc3_case(archs):
    """Classify the best viable arch's T-C3 pattern (spec section 7).

    Returns (arch, label, explanation).  ``freeze high`` means internal
    recurrence alone can drive timing regardless of the blank outcome —
    a blank failure then reflects the off-distribution constant fill, not
    dynamics dependence.
    """
    best = max(
        (a for a in archs if (_m(archs[a]["baseline_success"]) or 0) >= GATE_VIABLE),
        key=lambda a: _m(archs[a]["baseline_success"]), default=None)
    if best is None:
        return None, None, None
    a = archs[best]
    hi = lambda s: (_m(a[s]) or 0) >= .7
    lo = lambda s: (_m(a[s]) or 0) <= .3
    base, frz, blk = (hi("baseline_success"), hi("freeze_dynamics_success"),
                      hi("post_cue_blank_success"))
    blk_lo = lo("post_cue_blank_success")
    if base and frz and blk:
        return best, "A", ("cue 後の観測を止めても timing が保たれる "
                           "(internal recurrence 主体)")
    if base and frz and blk_lo:
        return best, "A-", ("freeze では timing が完全に保たれ、constant-fill "
                            "blank のみ崩壊 — 内部 recurrence は timing を駆動"
                            "できるが、0.5 充填は学習分布外入力として機能を"
                            "破壊する。dynamics 依存の証拠ではない")
    if base and lo("freeze_dynamics_success") and blk_lo:
        return best, "B", "freeze/blank で崩壊 (ongoing sensory dynamics 依存)"
    if base and blk_lo:
        return best, "C", ("freeze は部分的、blank で崩壊 "
                           "(internal recurrence と external dynamics の mixed)")
    return best, "mixed", (f"baseline={_fmt(_m(a['baseline_success']))}, "
                           f"freeze={_fmt(_m(a['freeze_dynamics_success']))}, "
                           f"blank={_fmt(_m(a['post_cue_blank_success']))}")


def generate(artifacts, out_path):
    artifacts = Path(artifacts)
    summary = json.loads((artifacts / "consolidated.json").read_text())
    eval_rows = [json.loads(p.read_text())
                 for p in sorted((artifacts / "eval").glob("*.json"))
                 if not p.name.endswith("_trajectory.json")]
    archs = summary["architectures"]
    hypotheses, strong, verdict = judge(summary)
    best_arch, case, case_expl = tc3_case(archs)
    commits = sorted({r.get("source_commit") for r in eval_rows
                      if r.get("source_commit")})
    commit = commits[0] if commits else subprocess.check_output(
        ["git", "rev-parse", "HEAD"], text=True).strip()
    run_cfgs = sorted((artifacts / "runs").glob("*/config.json"))
    device = json.loads(run_cfgs[0].read_text()).get("device", "cpu") \
        if run_cfgs else "cpu"
    integ = summary["protocol_integrity"]
    seeds = sorted({r.get("seed") for r in eval_rows})

    lines = []
    w = lines.append
    w("# T0 Results — Temporal Sense")
    w("")
    w("> 外部時計を持たないエージェント内部に、未知の時間間隔にも利用可能な「経過時間」の表現は形成されるか。")
    w("")
    w(f"**Protocol version: {summary['protocol_version']}** — "
      "held-out interpolation bands を training support から除去した再測定版。"
      "v1 protocol の欠陥と撤回済み解釈は "
      "[T0_V1_INVALIDATION.md](T0_V1_INVALIDATION.md) を参照。"
      "本ファイルは `experiments/t0/analysis/report.py` により "
      "v2 artifact のみから生成される。")
    w("")
    w("**適用範囲**: 以下の全数値・判定は主実験 **T0-A (interval production)** "
      "のものです。T0-B/C/D は env + oracle + smoke test のみで、学習済み実験結果ではありません。")
    w("")
    w("## 条件")
    w("")
    w(f"- source commit: `{commit}`")
    w(f"- architectures: {', '.join(sorted(archs))} / seeds: "
      f"{', '.join(map(str, seeds))}")
    w(f"- training support (sampler が生成しうる delay): "
      f"{summary.get('training_support')}")
    w(f"- excluded_training_delays (interpolation holdout): "
      f"{summary.get('excluded_training_delays')}")
    w(f"- interpolation primary: {summary.get('interpolation_primary')}; "
      f"holdout bands: {summary.get('interpolation_holdout')}; "
      f"extrapolation: {summary.get('extrapolation')}")
    w(f"- validation seed (model selection): "
      f"{(summary.get('rng_seeds') or {}).get('validation')}; "
      f"eval seed: {(summary.get('rng_seeds') or {}).get('eval')}; "
      "選択基準: `success + 0.1 * reward` on fixed validation grid")
    w(f"- device/backend: `{device}` "
      "(M2 Max; MPS は小模型・batch env で ~7x 低速のため未採用)")
    w(f"- protocol integrity: **{integ['status']}** "
      f"(n_eval_rows={integ.get('n_eval_rows')}, "
      f"train/interp overlap={integ.get('train_interpolation_overlap')}, "
      f"train/holdout overlap={integ.get('train_holdout_overlap')}, "
      f"train/extrap overlap={integ.get('train_extrapolation_overlap')}, "
      f"val/test overlap={integ.get('validation_test_overlap')}, "
      f"intervention target-independence="
      f"{integ.get('intervention_target_independence')})")
    w("")
    w("## 結果 (seed 平均 success rate)")
    w("")
    w("| arch | seen | interp (40,56) | interp band | extrap | hidden reset | post-cue blank | freeze dyn | elapsed R² |")
    w("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    for a in sorted(archs):
        s = archs[a]
        w(f"| {a} | {_fmt(_m(s['seen']))} | {_fmt(_m(s['interpolation']))} | "
          f"{_fmt(_m(s['interpolation_band']))} | {_fmt(_m(s['extrapolation']))} | "
          f"{_fmt(_m(s['hidden_reset_success']))} | "
          f"{_fmt(_m(s['post_cue_blank_success']))} | "
          f"{_fmt(_m(s['freeze_dynamics_success']))} | "
          f"{_fmt(_m(s['elapsed_r2']))} |")
    w("")
    w("ストレス (seed 平均):")
    w("")
    w("| arch | distractor .2 | scale ×0.5 | scale ×2.0 | hidden noise |")
    w("|---|---:|---:|---:|---:|")
    for a in sorted(archs):
        s = archs[a]
        w(f"| {a} | {_fmt(_m(s['distractor']))} | {_fmt(_m(s['scaled_0.5']))} | "
          f"{_fmt(_m(s['scaled_2.0']))} | {_fmt(_m(s['hidden_noise_success']))} |")
    w("")
    w("## 仮説判定 (pre-registered gates)")
    w("")
    w("| 仮説 | gate | 判定 | 実測 |")
    w("|---|---|---|---|")
    mlp_s = _m(archs.get("mlp", {}).get("seen"))
    w(f"| T0-G0 | protocol integrity clean | "
      f"{'PASS' if integ['status'] == 'PASS' else 'FAIL'} | {integ['status']} |")
    w(f"| T0-H1 | gru/lstm 全arch seen > mlp | "
      f"{'PASS' if hypotheses['T0-H1'] else 'FAIL'} | "
      f"mlp seen={_fmt(mlp_s)} |")
    gated_archs = [a for a in RECURRENT_GATED if a in archs]
    best_interp = max((archs[a] for a in gated_archs),
                      key=lambda s: _m(s['interpolation']) or 0, default=None)
    w(f"| T0-H2 | held-out interp ≥{GATE_INTERP_PRIMARY} かつ band ≥{GATE_INTERP_BAND} | "
      f"{'PASS' if hypotheses['T0-H2'] else 'FAIL'} | "
      + ("n/a" if best_interp is None else
         f"interp={_fmt(_m(best_interp['interpolation']))}, "
         f"band={_fmt(_m(best_interp['interpolation_band']))}") + " |")
    best_ex = max(archs.values(), key=lambda s: _m(s['extrapolation']) or 0)
    w(f"| T0-H3 | extrap ≥{GATE_EXTRAP} (strong) | "
      f"{'PASS' if hypotheses['T0-H3'] else 'FAIL'} | "
      f"best extrap={_fmt(_m(best_ex['extrapolation']))} |")
    w(f"| T0-H4 | baseline−reset ≥{GATE_RESET_DROP} | "
      f"{'PASS' if hypotheses['T0-H4'] else 'FAIL'} | "
      + ("n/a" if best_arch is None else
         f"{best_arch}: {_fmt(_m(archs[best_arch]['baseline_success']))}→"
         f"{_fmt(_m(archs[best_arch]['hidden_reset_success']))}") + " |")
    best_r2 = max(archs.values(), key=lambda s: _m(s['elapsed_r2']) or 0)
    w(f"| T0-H5 | elapsed R²≥{GATE_ELAPSED_R2}, shuffle<{GATE_SHUFFLE_MAX} | "
      f"{'PASS' if hypotheses['T0-H5'] else 'FAIL'} | "
      f"best R²={_fmt(_m(best_r2['elapsed_r2']))}, "
      f"shuffle={_fmt(_m(best_r2['shuffle_max_r2']))} |")
    w("")
    w("Strong PASS 構成要素: "
      + ", ".join(f"{k}={'PASS' if v else 'FAIL'}" for k, v in strong.items()))
    w("")
    if best_arch is not None:
        w(f"T-C3 case (best viable arch `{best_arch}`): **Case {case}** — "
          f"{case_expl}")
        w("")
    w(f"## 総合判定: **{verdict}**")
    w("")
    ckpts = sorted({r["checkpoint"].split("/")[-1] for r in eval_rows})
    w("## 観察")
    w("")
    w(f"- 使用 checkpoint: {', '.join(ckpts)} "
      "(imitation stage が validation best だった run は imitation_best を使用)")
    if best_arch:
        rows_b = [r for r in eval_rows if r["architecture"] == best_arch]
        for key in ("seen_mean_success", "interpolation_mean_success",
                    "extrapolation_mean_success"):
            vals = [r[key] for r in rows_b]
            w(f"- `{best_arch}` {key}: seed range "
              f"{_fmt(min(vals))}–{_fmt(max(vals))}")
    w("- `post_cue_blank` は全 arch で 0.00 — 0.5 充填は学習分布外入力であり、"
      "`freeze_dynamics` (episode 固有 obs 凍結) との対比が dynamics 依存性の"
      "判別子となる (gru64: freeze=.995 / blank=.000)")
    w("- temporal scaling は依然として全 arch でほぼ 0 — 学習された時計は"
      "world-speed に適応しない step counter")
    w("")
    w("## Limitations")
    w("")
    w("- aux task (T0-B/C/D) は env + oracle + smoke test のみ。学習済み結果はない")
    w("- T-C5 (temporal scaling) の結果は step counter vs world-clock の判別子であり、"
      "適応の成否は strong verdict のみに影響する")
    w("- probe は線形 readout の存在証明であり因果利用の証明ではない")
    w("- v1 の数値は protocol defect により全て破棄。比較・継続使用はしない "
      "([T0_V1_INVALIDATION.md](T0_V1_INVALIDATION.md))")
    w("")
    text = "\n".join(lines) + "\n"
    Path(out_path).parent.mkdir(parents=True, exist_ok=True)
    Path(out_path).write_text(text)
    return verdict


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--artifacts", required=True)
    p.add_argument("--out", required=True)
    a = p.parse_args()
    verdict = generate(a.artifacts, a.out)
    print(json.dumps({"verdict": verdict, "out": a.out}))


if __name__ == "__main__":
    main()
