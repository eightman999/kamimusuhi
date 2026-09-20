"""Generate the Japanese K0-F report from saved evidence, without opening models.

Missing or malformed gate evidence cannot produce research PASS. The operator's
final hardware audit is separate from the policy's measured-cost replay results.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import statistics
from datetime import datetime, timezone
from pathlib import Path

SECTIONS = (
    "結論", "研究質問", "実装", "Mac sensors", "llm_master sensors", "InteroceptiveFrame",
    "workload", "prediction probe", "learning protocol", "BODY vs BLIND", "BODY vs SHUFFLED",
    "BODY vs STALE", "counterfactual body", "temporal OOD", "sensor OOD", "safety", "statistics",
    "limitations", "success criteria", "Kamimusuhiへの意味", "次phase判断", "reproducibility",
    "runtime cleanup", "commit一覧")
PLOTS = ("body_timeseries.png", "thermal_load_relationship.png", "resource_pressure.png", "network_body_state.png",
         "prediction_probe.png", "body_ablation.png", "counterfactual_body.png", "ood_heatmap.png",
         "policy_action_by_body_state.png", "pareto.png")


def finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def f(value, digits=6):
    if value is None:
        return "未取得"
    if isinstance(value, bool):
        return "PASS" if value else "FAIL"
    if finite(value):
        return f"{value:.{digits}g}"
    if isinstance(value, float):
        return "無効値"
    if isinstance(value, (list, dict)):
        return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return str(value).replace("|", "\\|").replace("\n", " ")


def timestamp_text(value):
    return datetime.fromtimestamp(value, timezone.utc).isoformat(timespec="seconds") if finite(value) else "未取得"


def table(headers, records):
    records = list(records)
    if not records:
        return "未取得・未評価。"
    return "\n".join(["| " + " | ".join(headers) + " |", "| " + " | ".join("---" for _ in headers) + " |"] +
                     ["| " + " | ".join(f(value) for value in row) + " |" for row in records])


def rows(value):
    if isinstance(value, list):
        return value
    return value.get("rows", []) if isinstance(value, dict) and isinstance(value.get("rows", []), list) else []


def mapping(value):
    return value if isinstance(value, dict) else {}


def criterion(record):
    """Recheck the numerical prespecified criterion; a saved true flag alone fails."""
    record = mapping(record)
    n, mean, ci, p = record.get("n_seeds"), record.get("mean_difference", record.get("mean")), record.get("ci95"), record.get("exact_sign_p")
    return (record.get("pass") is True and isinstance(n, int) and n >= 8 and finite(mean) and mean > 0
            and isinstance(ci, list) and len(ci) == 2 and all(finite(x) for x in ci) and 0 < ci[0] <= ci[1]
            and finite(p) and 0 <= p <= .05)


def comparison_table(comparisons, keys):
    result = []
    for name in keys:
        record = mapping(comparisons.get(name))
        ci = record.get("ci95")
        result.append((name, record.get("n_seeds"), record.get("mean_difference", record.get("mean")), record.get("sd"),
                       record.get("median"), f"[{f(ci[0])}, {f(ci[1])}]" if isinstance(ci, list) and len(ci) == 2 else "未取得",
                       record.get("exact_sign_p"), "PASS" if criterion(record) else "FAIL / 未達"))
    return table(("paired 比較", "seed n", "平均差", "標本 SD", "中央値", "95% CI", "exact sign p", "判定"), result)


def mean_records(records, key):
    values = [record.get(key) for record in records if finite(record.get(key))]
    return statistics.mean(values) if values else None


def paired_seed_coverage(records, left, right, *, replay=False):
    groups = []
    for mode in (left, right):
        selected = [r for r in records if r.get("mode") == mode and (not replay or r.get("architecture") == "GRU128" and r.get("training_mode") == "BODY")]
        seeds = [r.get("seed") for r in selected]
        if len(seeds) < 8 or any(not isinstance(seed, int) for seed in seeds) or len(seeds) != len(set(seeds)):
            return False
        if not all(finite(r.get("utility")) for r in selected):
            return False
        groups.append(set(seeds))
    return groups[0] == groups[1]


def metric_table(records, *, architecture="GRU128", include_training=True):
    selected = [record for record in records if record.get("architecture", architecture) == architecture]
    groups = {}
    for record in selected:
        mode = record.get("mode", "unknown")
        if include_training and record.get("training_mode") == "BLIND":
            mode = "TRAINED_BLIND（独立学習）"
        groups.setdefault(mode, []).append(record)
    return table(("条件", "seed n", "効用", "遅延 秒", "締切内成功率", "実行失敗率"),
                 ((mode, len({r.get("seed") for r in group}), mean_records(group, "utility"), mean_records(group, "latency_seconds"),
                   mean_records(group, "deadline_success_rate"), mean_records(group, "failure_rate")) for mode, group in sorted(groups.items())))


class Evidence:
    def __init__(self, primary, policy=None, live=None):
        self.primary = Path(primary)
        self.policy = Path(policy) if policy else self.primary
        self.live = Path(live) if live else self.primary
        self.inputs = {}
        self.errors = []

    def read(self, filename, scope="primary"):
        path = getattr(self, scope) / filename
        if not path.exists():
            return None
        try:
            raw = path.read_bytes()
            def reject_nonfinite(value):
                raise ValueError(f"non-finite JSON constant: {value}")
            decode = lambda content: json.loads(content, parse_constant=reject_nonfinite)
            value = [decode(line) for line in raw.splitlines() if line.strip()] if path.suffix == ".jsonl" else decode(raw)
        except (OSError, ValueError):
            self.errors.append(f"{scope}/{filename}: 読み取り失敗")
            return None
        self.inputs[scope + "/" + filename] = hashlib.sha256(raw).hexdigest()
        return value


def telemetry_summary(records):
    records = rows(records)
    timestamps = [row.get("timestamp") for row in records]
    valid = bool(records) and all(finite(t) for t in timestamps)
    monotonic = valid and all(right > left for left, right in zip(timestamps, timestamps[1:]))
    kinds = sorted({row.get("source_kind", "unknown") for row in records})
    return {"samples": len(records), "span_seconds": timestamps[-1] - timestamps[0] if monotonic else None,
            "strictly_increasing": bool(monotonic), "source_kinds": kinds,
            "real_stream": len(records) >= 2 and monotonic and kinds == ["real"],
            "max_interval_seconds": max((b-a for a, b in zip(timestamps, timestamps[1:])), default=None) if monotonic else None}


def raw_metric_summary(records, keys):
    result = []
    records = rows(records)
    for key in keys:
        observed = []
        for row in records:
            value = mapping(row.get("metrics")).get(key)
            quality = mapping(row.get("quality")).get(key, 1 if value is not None else 0)
            if finite(value) and finite(quality) and quality > 0:
                observed.append(value)
        result.append((key, len(observed), statistics.mean(observed) if observed else None, min(observed) if observed else None, max(observed) if observed else None))
    return table(("生 sensor / 単位はキー末尾", "有効 sample", "記述平均", "最小", "最大"), result)


def frame_summary(records):
    records = rows(records)
    valid = 0
    for frame in records:
        arrays = [frame.get(key) for key in ("values", "mask", "quality", "age_s")]
        if not all(isinstance(array, list) and len(array) == 20 for array in arrays):
            continue
        values, masks, quality, ages = arrays
        if (all(finite(x) and 0 <= x <= 1 for x in values + masks + quality)
                and all(mask in (0, 1) and (q > 0 if mask else q == 0 and value == 0) for value, mask, q in zip(values, masks, quality))
                and all(x is None or finite(x) and x >= 0 for x in ages)
                and frame.get("normalization_identity") and frame.get("provenance") and finite(frame.get("timestamp"))):
            valid += 1
    return {"frames": len(records), "valid_fixed_frames": valid, "all_valid": bool(records) and valid == len(records),
            "normalization_identities": sorted({str(frame.get("normalization_identity")) for frame in records}),
            "schema_versions": sorted({str(frame.get("schema_version")) for frame in records}),
            "mean_availability": mean_records(records, "availability")}


def best_validation_core(records):
    groups = {}
    for row in rows(records):
        if row.get("training_mode") == "BODY" and finite(row.get("best_validation_utility")):
            groups.setdefault(row.get("architecture"), []).append(row)
    valid = {arch: group for arch, group in groups.items() if arch and len(group) >= 8 and len({r.get("seed") for r in group}) == len(group)}
    if not valid:
        return {"architecture": None, "selection": "validation only", "reason": "8独立seedの BODY 学習結果を確認できない"}
    architecture = max(sorted(valid), key=lambda arch: mean_records(valid[arch], "best_validation_utility"))
    return {"architecture": architecture, "selection": "validation only", "n_seeds": len(valid[architecture]),
            "mean_validation_utility": mean_records(valid[architecture], "best_validation_utility"),
            "architectures_evaluated": sorted(valid)}


def generate(artifacts, policy_artifacts=None, live_artifacts=None, output=None):
    evidence = Evidence(artifacts, policy_artifacts, live_artifacts)
    mac = evidence.read("raw_mac_telemetry.jsonl")
    master = evidence.read("raw_master_telemetry.jsonl")
    frames = evidence.read("interoceptive_frames.jsonl")
    aligned = evidence.read("aligned_body_telemetry.jsonl")
    normalization = mapping(evidence.read("normalization_config.json"))
    workload = evidence.read("workload_manifest.json")
    record_only_workloads = evidence.read("record_only_workload_manifest.json")
    config = mapping(evidence.read("training_config.json", "policy"))
    run_summary = mapping(evidence.read("run_summary.json", "policy"))
    probe = mapping(evidence.read("prediction_probe.json", "policy"))
    ablation = mapping(evidence.read("ablation_results.json", "policy"))
    counter = mapping(evidence.read("counterfactual_body.json", "policy"))
    ood = mapping(evidence.read("ood_results.json", "policy"))
    finals = mapping(evidence.read("final_checkpoint_results.json", "policy"))
    live = mapping(evidence.read("live_results.json", "live"))
    live_config = mapping(evidence.read("live_config.json", "live"))
    resources = mapping(evidence.read("resource_summary.json"))
    runtime = mapping(evidence.read("final_runtime_state.json"))
    baseline = mapping(evidence.read("baseline_integrity.json"))
    attempts = evidence.read("acquisition_attempts.json")
    commits = evidence.read("commits.json")
    validation = mapping(evidence.read("validation_results.json"))
    mac_info, master_info, frame_info = telemetry_summary(mac), telemetry_summary(master), frame_summary(frames)
    comparisons = mapping(ablation.get("comparisons"))
    cf_comparison = mapping(mapping(counter.get("comparisons")).get("GRU128"))
    live_comparisons = mapping(live.get("comparisons"))
    probe_gate = mapping(probe.get("gate"))
    experiment_scope = config.get("experiment_scope", "confirmatory")
    exploratory = experiment_scope == "exploratory_after_failed_probe"
    keys = ["GRU128_BODY_vs_" + mode for mode in ("BLIND", "SHUFFLED", "STALE")]
    gates = {
        "A_telemetry": resources.get("telemetry_continuous") is True and mac_info["real_stream"] and master_info["real_stream"],
        "B_fixed_frame": resources.get("fixed_frame_valid") is True and frame_info["all_valid"] and bool(normalization) and bool(rows(aligned)),
        "C_BODY_vs_BLIND": criterion(comparisons.get(keys[0])) and paired_seed_coverage(rows(ablation), "BODY", "BLIND", replay=True),
        "D_BODY_vs_SHUFFLED": criterion(comparisons.get(keys[1])) and paired_seed_coverage(rows(ablation), "BODY", "SHUFFLED", replay=True),
        "E_BODY_vs_STALE": criterion(comparisons.get(keys[2])) and paired_seed_coverage(rows(ablation), "BODY", "STALE", replay=True),
        "F_counterfactual": criterion(cf_comparison) and len({row.get("seed") for row in rows(counter) if row.get("architecture") == "GRU128" and row.get("available") is True}) >= 8 and any(finite(row.get("action_change_rate")) and row["action_change_rate"] > 0 for row in rows(counter) if row.get("architecture") == "GRU128"),
        "G_live_BODY_vs_BLIND": criterion(live_comparisons.get("BODY_vs_BLIND")) and paired_seed_coverage(rows(live), "BODY", "BLIND"),
        "G_live_BODY_vs_TRAINED_BLIND": criterion(live_comparisons.get("BODY_vs_TRAINED_BLIND")) and paired_seed_coverage(rows(live), "BODY", "TRAINED_BLIND"),
        "H_safety": resources.get("safety_pass") is True,
        "I_reproducibility": resources.get("reproducibility_pass") is True and bool(config.get("identities")) and bool(normalization),
        "baseline_preservation": baseline.get("pass") is True,
        "runtime_cleanup": runtime.get("cleanup_pass") is True,
        "probe_validation_gate": probe_gate.get("pass") is True,
        "confirmatory_scope": not exploratory,
        "evidence_readable": not evidence.errors,
    }
    criteria = {"schema_version": "k0f.success.v1", "research_status": "PASS" if all(gates.values()) else "FAIL",
                "experiment_scope": experiment_scope,
                "gates": gates, "unmet": [key for key, value in gates.items() if not value],
                "rule": "prespecified conjunction; missing evidence is not success; every paired gate requires n>=8, mean>0, 95% CI lower>0, exact sign p<=.05",
                "live_scope": "new real jobs after frozen policy choices; separately reported from measured-cost replay"}
    best = best_validation_core(run_summary)
    execution_complete = bool(resources.get("execution_complete") is True and runtime.get("cleanup_pass") is True and baseline.get("pass") is True and not evidence.errors)
    statistics_output = {"schema_version": "k0f.report_statistics.v1", "execution_complete": execution_complete,
                         "experiment_scope": experiment_scope,
                         "research_status": criteria["research_status"], "best_core_validation_only": best,
                         "telemetry": {"mac": mac_info, "master": master_info}, "frame": frame_info,
                         "probe_gate": probe_gate, "replay_comparisons": comparisons, "counterfactual_comparison": cf_comparison,
                         "acquisition_attempts": attempts,
                         "record_only_workloads": record_only_workloads,
                         "live_comparisons": live_comparisons, "source_hashes": evidence.inputs, "read_errors": evidence.errors,
                         "statistical_unit": "training seed; shared heldout workloads, not 1 Hz samples"}
    sections = []
    def section(number, text):
        sections.append(f"## {number}. {SECTIONS[number-1]}\n\n{text}\n")

    missing_gates = "、".join(criteria["unmet"]) or "なし"
    section(1, f"**研究成功: {criteria['research_status']}。実施完了: {'確認済み' if execution_complete else '未確認 / 作業残あり'}。**\n\n"
            + ("**本学習・ablation・OOD・liveは、予測probe不合格後の探索的診断（exploratory_after_failed_probe）である。後続比較が良好でも研究全体FAILを固定し、確証的な研究成功へ昇格しない。**\n\n" if exploratory else "") +
            f"未達・未検証 gate: {missing_gates}。最良 Core は validation の seed 平均だけで選定し、{best.get('architecture') or '未学習 / 未選定'}。\n\n"
            "実測テレメトリーの取得、保存済み実測費用による再生評価、凍結方策が選んだ資源で新規に実行した実ジョブを別の証拠層として示す。GUI の完成を研究成功に含めない。\n\n" +
            comparison_table(comparisons, keys) + "\n\n新規実ジョブの主確認:\n\n" + comparison_table(live_comparisons, ("BODY_vs_BLIND", "BODY_vs_TRAINED_BLIND")))
    section(2, "現在の正しい機械身体情報を持つ非言語 Core が、身体情報を持たない Core より、資源選択と下流の task utility を改善するか。必要な因果鎖は「正しい身体情報 → 意思決定の変化 → 下流効用の改善」。温度分類や行動変化だけでは成功としない。")
    section(3, "独立 namespace `experiments/k0_f_interoception` に、Swift Mac sensor、Linux Python sensor、SSH private transport、timestamp 整列、20次元正規化、44次元 policy 入力、GRU policy、介入評価、閲覧専用日本語 GUI を実装した。K0-E/E2 は入力互換や checkpoint の保全対象とし、既存実験を上書きしない。\n\n"
            "GUI は保存済み raw 値、mask、取得時 age、quality、Core action/hidden norm、温度・使用率・VRAM・メモリー圧・RTT・行動時系列を表示する。観測未取得・未評価を正常値や成功値で補完しない。\n\n検証結果 artifact: " + f(validation))
    section(4, table(("指標", "値"), (("real samples", mac_info["samples"]), ("取得時間 秒", mac_info["span_seconds"]), ("最大間隔 秒", mac_info["max_interval_seconds"]), ("時刻単調増加", mac_info["strictly_increasing"]))) +
            "\n\nFoundation thermalState、Mach CPU/VM、IOKit 電源、sysctl swap、ICMP RTT を privilege 不要で取得する。Mac の生温度・fan は安定公開 API がないため任意・未取得。熱圧は nominal/fair/serious/critical を 0..1 に encode。memory pressure は VM 使用圧 proxy で OS の pressure event そのものではない。\n\n" +
            raw_metric_summary(mac, ("thermal_pressure", "cpu_utilization", "memory_pressure", "battery_fraction", "network_rtt_ms", "network_loss", "daemon_cpu_fraction", "daemon_rss_bytes")))
    section(5, table(("指標", "値"), (("real samples", master_info["samples"]), ("取得時間 秒", master_info["span_seconds"]), ("最大間隔 秒", master_info["max_interval_seconds"]), ("時刻単調増加", master_info["strictly_increasing"]))) +
            "\n\n/proc、sysfs、nvidia-smi から CPU 温度・使用率・iowait・RAM・I/O・RTX3060/P100 の温度、利用率、VRAM、電力を取得する。Mac→中央と中央→Mac の RTT は別 sensor。取得できない向きの値は欠損として残す。\n\n" +
            raw_metric_summary(master, ("cpu_temperature_c", "cpu_utilization", "memory_pressure", "io_pressure", "rtx3060_temperature_c", "rtx3060_utilization", "rtx3060_power_w", "p100_temperature_c", "p100_utilization", "p100_power_w", "daemon_cpu_fraction", "daemon_rss_bytes")))
    section(6, table(("固定 frame 監査", "値"), frame_info.items()) + "\n\n20値 + 20 mask を保存し、task 4値と合わせて44 floatを policyへ渡す。欠損値0には必ずmask0を伴わせる。sensorごとのage/quality、source/receipt timestamp、sequence、正規化identityを保存し、raw/aligned/frame/policy-inputを別ファイルにする。心理ラベル・workload名・絶対時刻・cost・teacherをpolicy入力へ入れない。\n\n正規化設定: " + f(normalization))
    workloads = rows(workload)
    splits = {}
    for row in workloads:
        split = str(row.get("split", "未指定"))
        splits[split] = splits.get(split, 0) + 1
    section(7, f"保存 workload block 数: {len(workloads)}。split件数: {f(splits)}。\n\n"
            "事前計画は48 block、train24 / validation8 / test16。idle、CPU、RTX3060、P100、dual GPU、mixed、disk I/O、network transferを含み、split境界に35秒の記録区間を置く。固定4種類の行列乗算を CPU/RTX3060/P100 で無作為順に測定する。計算 job は実処理だが言語理解の代理ではない。実LLM workloadのrecord-only証拠は別収集・別artifactで確認する。\n\n"
            "1回ずつの action別測定は短時間のpaired測定であり、同時の物理反実仮想ではない。収集 sample 数を独立実験数と数えない。\n\n"
            "初回primary収集は24block後のMac sleep（壁時計約805秒、monotonic約1秒）により安全停止した。中断記録を保持し、driver期間中のidle-sleep抑制を追加して同条件のprimary_v2を再取得した。初回中断データは今回のprimary集計・独立nに混ぜない。\n\n取得試行の監査記録: " + f(attempts) +
            "\n\n既存J72による実LLMの独立record-only区間:\n\n" +
            table(("workload", "model", "source", "開始", "終了", "request数", "record-only", "PID", "終了コード"),
                  ((r.get("workload_label"), r.get("model"), r.get("source_kind"), timestamp_text(r.get("start_timestamp", r.get("start"))),
                    timestamp_text(r.get("end_timestamp", r.get("end"))), r.get("requests"), "はい" if r.get("record_only") else "いいえ", r.get("pid"), r.get("returncode")) for r in rows(record_only_workloads))) +
            "\n\nこの実LLM区間はtelemetry取得の観測証拠として別保存する。primary_v2の学習・validation・held-out・seed統計には混ぜず、行列計算policyの言語能力評価とは扱わない。")
    section(8, "ridge λ=10、特徴標準化・target SDはtrainだけでfit。初回v1はBODY 1.323946 / BLIND 0.607069でFAIL。I/O特徴のtrain SDが約0.00005894と微小で、validation変動が最大62.59 SDへ増幅されていた。test成績を見ずにbody特徴のSD下限0.05だけを一度修正し、target・λ・10%gateを維持したv2もFAILだった。未来10秒のGPU使用率と、次jobの実測最小完了時間を区別する。validationで有効非定数targetが2種類以上かつ BODY の正規化MAEが BLIND より10%以上低いことを学習開始 gate とする。\n\n" +
            table(("validation gate", "値"), (("PASS", probe_gate.get("pass")), ("相対 MAE 改善", probe_gate.get("relative_mae_improvement")), ("target", probe_gate.get("targets")), ("BODY / BLIND / SHUFFLED / STALE MAE", probe_gate.get("normalized_mae")), ("test probe 実施", "実施" if probe.get("test_evaluated") is True else "未実施（validation gate未達）"))) +
            ("\n\nvalidation gate が通っていないため、後続policy・ablation・OOD・liveは探索的診断として区別する。v1不合格を保存し、標準化SDのfloorを0.05へ修正した一回のv2も不合格。成功閾値を変更せず、追加probe探索で成功を探さない。後続成績にかかわらず研究全体はFAIL。" if exploratory else
             "\n\nvalidation gate が通っていないため policy 学習・held-out評価の成功は主張しない。未実施は未実施として記録する。" if probe_gate.get("pass") is not True else "\n\nprobe の gate は記述的な事前screening。これ単独で研究成功や身体情報の因果価値とは呼ばない。"))
    section(9, ("**探索的診断として実施。予測probe不合格後の実験であり、事前の学習開始gateを通過した確証試験ではない。研究全体FAILを固定する。別学習BLINDを含む全結果を表示し、良好な条件だけを採用しない。**\n\n" if exploratory else "") +
            table(("保存 training config", "値"), ((key, ("実施" if config.get(key) else "未実施") if key in ("PPO", "DAgger") else config.get(key)) for key in ("experiment_scope", "seeds", "hidden_sizes", "epochs", "batch_size", "learning_rate", "device", "selection", "PPO", "DAgger", "source_commit"))) +
            "\n\nGRU128 BODY と独立 BLIND を同一seed・初期重み・task順で教師学習。utilityは `1-min(latency/deadline,2)`、実行失敗は-1。lossは teacher cross entropy + expected utility regret。1 episodeに1資源選択であり、replay actionは次rowを変えないので DAggerは実施しない。PPOも実施しない。validation utility最大の最初のbestと最終epochを別保存する。\n\n" +
            table(("validation-only 選定", "値"), best.items()) + "\n\nGRU64: " + ("保存runあり。" if any(row.get("architecture") == "GRU64" for row in rows(run_summary)) else "未実施。軽量構造との優越性は未検証。") +
            "\n\nfinal checkpoint は `final_checkpoint_results.json` に別保存し、primaryをbestから置換しない。final比較の保存行数: " + str(len(rows(finals))))
    for number, mode, rationale in ((10, "BLIND", "同じ BODY-best checkpoint への全body mask介入。情報除去による入力分布変化の影響があるため、独立学習BLINDとの差も別示する。"),
                                     (11, "SHUFFLED", "同split・同履歴長の別workload blockのbodyを全単射で交換し、task列・cost列・GRU更新回数・bodyの周辺分布を保持する。現在状態との対応を壊した対照。"),
                                     (12, "STALE", "実timestampで30秒前のbodyを使い、staleness channelには実際の古さを通知するage-aware対照。欠損はmask0。単なるbody channelの有無と、現在性の寄与を分ける。")):
        text = rationale + "\n\n" + comparison_table(comparisons, ("GRU128_BODY_vs_" + mode,))
        if number == 10:
            text += "\n\n保存実測費用の再生評価:\n\n" + metric_table(rows(ablation))
            text += "\n\n独立学習した BLIND との secondary 比較:\n\n" + comparison_table(comparisons, ("GRU128_BODY_vs_independently_trained_BLIND",))
            text += "\n\n凍結方策が選んだ資源で新規実行したジョブ:\n\n" + metric_table(rows(live), include_training=False) + "\n\n" + comparison_table(live_comparisons, ("BODY_vs_BLIND", "BODY_vs_TRAINED_BLIND"))
            text += "\n\nliveは同seed/task直前bodyを共通にし、5modeの実行順を無作為化する。実行時間は新規測定で、保存costの再使用ではない。共通bodyの取得から後続mode実行までの物理状態変化は残る。"
        else:
            text += "\n\n新規実ジョブでの補助比較:\n\n" + comparison_table(live_comparisons, ("BODY_vs_" + mode,))
        section(number, text)
    section(13, "同じtask・重み・Aの過去履歴・hiddenをcloneし、最終bodyだけA/Bへ分岐する。対応bodyのactionとAの固定actionをBの実測costで採点する。全ての同task・異block ordered pairを対象とし、改善するpairだけを選ばない。\n\n" +
            comparison_table({"GRU128": cf_comparison}, ("GRU128",)) + "\n\n" +
            table(("構造", "seed", "pair数", "行動変化率", "matched効用", "frozen効用", "効用差"),
                  ((r.get("architecture"), r.get("seed"), r.get("n_pairs"), r.get("action_change_rate"), r.get("matched_utility"), r.get("frozen_utility"), r.get("utility_gain")) for r in rows(counter))) +
            "\n\nモデル内部のbody入力への介入と保存済み実測outcomeの再生であり、物理身体そのものを無作為化した実験ではない。行動変化だけでPASSにしない。")
    for number, category in ((14, "temporal"), (15, "sensor")):
        selected = [r for r in rows(ood) if r.get("category") == category]
        summary = metric_table(selected, include_training=False)
        groups = {mode: [r for r in selected if r.get("mode") == mode] for mode in {r.get("mode") for r in selected}}
        valid_groups = {mode: group for mode, group in groups.items() if mean_records(group, "utility") is not None}
        worst = min(valid_groups, key=lambda mode: mean_records(valid_groups[mode], "utility")) if valid_groups else None
        note = "sampling interval、更新遅延、jitter、dropout、30秒STALE、task履歴長変更をcontrolled perturbationとして評価。固定step暗記や実時間での頑健性を当然とは扱わない。" if category == "temporal" else "noise、dropout、constant、scale、GPU sensor欠損、Mac欠損、network latency、inversion、permutationを評価。controlled_resource_unavailableだけはRTX選択時のfailureを合成した資源障害simulationで、他の観測介入と分ける。実GPUを故障・停止させた試験ではない。"
        section(number, note + "\n\n" + summary + f"\n\n最悪の保存条件: {worst or '未評価'}、平均効用 {f(mean_records(valid_groups[worst], 'utility')) if worst else '未取得'}。大幅低下も省略しない。")
    section(16, "背景GPU duty<=50%、256MiB/device未満のmatrix、CPU背景<=2thread、GPU75℃以上または必須観測不能で停止する。OOM storm・thermal shutdown・電源断・host crashを目標にしない。センサー取得だけで安全運用の完了とはしない。\n\n" +
            table(("最終安全監査", "値"), (("安全判定", resources.get("safety_pass")), ("既存成果物の保全", baseline.get("pass")),
                  ("連続telemetry", resources.get("telemetry_continuous")), ("固定frame検証", resources.get("fixed_frame_valid")),
                  ("runtime cleanup", runtime.get("cleanup_pass")))) +
            "\n\n実測温度・負荷・daemon overheadの記述統計は4–5節を参照。全監査値と資源別の詳細は [resource_summary.json](resource_summary.json)、終了時の状態は [final_runtime_state.json](final_runtime_state.json) に保存する。")
    section(17, "独立単位は training seed 0–7。同seed・同評価task列の平均utility差をpairedで比較し、mean・sample SD・median・20,000回seed bootstrap 95% CI・両側exact sign pを保存する。各primaryは n>=8、mean>0、CI下限>0、p<=.05の積条件。telemetry sampleや同一seedのepisodeを独立nとして増やさない。\n\n"
            "CIは固定held-out workloadを共有した学習seed変動だけを表す。未知workload母集団や長期環境変動は含まない。複数比較のpは未補正で、事前指定primaryは全条件成立を要求し、補助比較は探索的に解釈する。\n\n統計機械可読値: `report_statistics.json`、`ablation_results.json`、`live_results.json`。")
    section(18, "資源別コスト行列は順次実測で同時反実仮想ではない。replayで良好でも実ジョブの有効性は別検証が必要。新規liveも同一装置・有限workload cohortであり、一般的な身体・未知機械・長期日常環境への汎化を証明しない。\n\n"
            "低次元の負荷・圧力はproxyを含む。Mac生温度は未取得で、memory proxyとOS pressureは異なる。sensor permutation/noise等は合成介入、実ネットワーク障害・GPU故障・危険温度試験ではない。行列計算jobの効用改善を言語能力や主観的感覚の証拠へ拡張しない。実ジョブのfinite/checksum検査は出力行列の最初の1行を対象とし、全要素の数値正当性検査ではない。独立double参照との検査行checksum差は最大7.76e-7だった。GRU64・PPO・MASTER_ONLY/MAC_ONLYの独立比較は未実施で、その優劣やMac追加価値は未確認。Mac欠損の合成介入だけで末梢ノードの追加価値を証明しない。\n\n"
            "無負荷・高負荷の区別だけで成功としない。現在の正しいbodyとdownstream outcomeの改善が成立しないときは研究FAILを維持する。")
    section(19, table(("成功条件", "判定"), gates.items()) + f"\n\n全条件の結合: **{criteria['research_status']}**。欠損・読取失敗・未実施は未検証でありPASSへ置換しない。")
    section(20, ("保存実測再生の3比較、反実仮想、凍結Coreからの新規実ジョブ、安全・再現性の全gateが成立した。限定された計算資源選択taskにおいて、現在身体情報への因果依存と下流効用改善を確認した。主観的感覚や一般知能を示すものではない。" if criteria["research_status"] == "PASS" else
                 "機械身体情報を取得・正規化・入力・検証する実験基盤と、研究上成立した条件を残す。全gateが成立していないため、「身体感覚を獲得した」「実資源判断を有意に改善した」という総合的な成功主張はしない。失敗条件からsensor設計・学習・評価のどこを改善すべきか判断する。"))
    section(21, ("現段階で次phaseへの昇格は推奨しない。SHUFFLED差と新規実ジョブの独立BLIND差、probeを新しい独立データと事前固定した設計で再検証することを優先する。今回のtestを使う追加調整は行わない。\n\n" if criteria["research_status"] == "FAIL" else "") + "K0-G Visual Peripheral Sense、K0-H Auditory Peripheral Senseは次候補として挙げるに留める。camera・microphone・gaze・VAD等を自動実装しない。今回の結果、未達gate、安全・runtime cleanupを確認した後、次phaseへの最終承認を一度だけユーザーへ求める。")
    section(22, "source commit、source file hash、dataset hash、normalization identity、schema、seed、best/final checkpoint hashを保存する。rawは実測を維持し、synthetic介入と別管理する。checkpointバイナリを無条件でGitへ追加しない。接続情報はGit除外済みprivate設定から取得し、報告書・raw・学習configへ記録しない。\n\n" +
            table(("再現性", "値"), (("source_commit", config.get("source_commit")), ("training identities", config.get("identities")), ("live checkpoint identities", live_config.get("checkpoint_sha256")), ("baseline pass", baseline.get("pass")), ("reproducibility_pass", resources.get("reproducibility_pass")))) +
            "\n\n保存結果からの再生成:\n\n```sh\npython -m experiments.k0_f_interoception.visualize --artifacts ARTIFACTS\npython -m experiments.k0_f_interoception.report --artifacts ARTIFACTS --policy-artifacts POLICY --live-artifacts LIVE\n```\n\n収集・学習の正確な引数と停止手順は実験READMEを参照。必須図: " + "、".join(f"[{name}]({name})" for name in PLOTS) + "。")
    section(23, table(("最終runtime", "値"), ((key, ("はい" if value else "いいえ") if isinstance(value, bool) and key != "cleanup_pass" else value) for key, value in runtime.items())) + "\n\n停止したことと負荷・温度の通常範囲への復帰を別々に確認する。今回所有したsensor・training worker・背景workload・transportのみを終了し、既存serviceを開始前状態へ戻す。無関係なGPU processを停止しない。停止証拠がなければcleanupをPASSにしない。")
    commit_rows = rows(commits)
    section(24, table(("commit", "内容"), ((row.get("hash"), row.get("subject")) for row in commit_rows)) if commit_rows else
            "実装・実験単位でcommitし、外部pushは行わない。最終結果commitを含む一覧は、親agentが最終commit後に `commits.txt` と最終回答へ追記する。生成時点の学習source commit: " + f(config.get("source_commit")))
    result = "# K0-F Machine Interoception / Minimal Embodiment 結果報告\n\n" + "\n".join(sections)
    destination = Path(output) if output else evidence.primary
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "K0_F_REPORT.md").write_text(result)
    for name, value in (("report_statistics.json", statistics_output), ("success_criteria.json", criteria)):
        (destination / name).write_text(json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n")
    return statistics_output, criteria


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifacts", required=True, type=Path)
    parser.add_argument("--policy-artifacts", type=Path)
    parser.add_argument("--live-artifacts", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    stats, criteria = generate(args.artifacts, args.policy_artifacts, args.live_artifacts, args.output)
    print(json.dumps({"execution_complete": stats["execution_complete"], "research_status": criteria["research_status"], "unmet": criteria["unmet"]}, ensure_ascii=False))
