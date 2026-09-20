"""Generate Japanese E2 evidence reports exclusively from saved experimental data.

No training, endpoint calls, model selection on test data, or K0-E writes occur.
Missing evidence remains explicit; execution completion and research success differ.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import math
import re
from collections import Counter
from pathlib import Path

import numpy as np

ARCHES=("mlp","gru64","gru128")
NAMES={"mlp":"MLP","gru64":"GRU64","gru128":"GRU128"}
REQUIRED=["run_summary.json","policy_interventions.json","language_response_ablation.json","retention_results.json","ood_results.json","voi_results.json","counterfactual.json","ambiguity_audit.json","active_sensing.json","habituation_results.json","ppo_ablation.json","seed_statistics.json","j72_results.json","run_evidence.json","validation.json","leakage_audit.json","trained_delay_ablation.json"]
PLOTS=["voi_cost_curve.png","voi_performance_curve.png","reliability_curve.png","latency_curve.png","policy_intervention.png","response_ablation.png","retention_curve.png","ppo_ablation.png","ood_heatmap.png","pareto.png"]
SCENARIOS={"language":"言語曖昧性","memory":"厳密な記憶","orient":"能動ORIENT","observe":"能動OBSERVE","habituation":"慣れ","irrelevant":"無関係刺激"}
METRICS={"task_success":"課題成功率","scenario_macro_success":"シナリオmacro成功率","reward":"1エピソード累積報酬","oracle_regret":"oracle参照との差（regret）","llm_call_rate":"受理された言語callを含むepisode割合","required_call_recall":"必要call再現率","required_call_precision":"必要call適合率","required_call_f1":"必要call F1","false_call_rate":"不要episodeでのcall割合","missed_required_rate":"必要episodeでの見逃し率","orient_rate":"ORIENT選択率/step","observe_rate":"OBSERVE選択率/step","recall_rate":"RECALL選択率/step","action_macro_f1":"行動macro-F1","action_entropy_nats":"行動entropy（nat）","resource_cost":"資源cost/episode","language_cost":"言語cost/episode","physical_language_cost":"物理的言語cost/episode","latency_cost":"応答待ちcost/episode","wait_action_cost":"WAIT行動cost/episode","memory_retention":"厳密記憶成功率","habituation_sequence_success":"慣れ系列成功率","novelty_reaction":"新奇刺激反応率","language_latency_steps":"言語情報到着までの遷移数","params":"parameter数","active_connections":"論理active connection数","cpu_inference_ms_mean":"CPU推論平均ms","cpu_inference_ms_median":"CPU推論中央値ms","cpu_inference_ms_p95":"CPU推論p95 ms","training_steps_per_second_mean":"学習transitions/秒"}


def finite(value):return isinstance(value,(int,float)) and not isinstance(value,bool) and math.isfinite(value)


def statistics(values):
    values=[float(v) for v in values if finite(v)]
    if not values:return dict(n_seeds=0,mean=None,sd=None,median=None,ci95=None)
    array=np.asarray(values);rng=np.random.default_rng(314159)
    means=rng.choice(array,size=(4000,len(array)),replace=True).mean(1)
    return dict(n_seeds=len(values),mean=float(array.mean()),sd=float(array.std(ddof=1)) if len(array)>1 else None,
                median=float(np.median(array)),ci95=[float(x) for x in np.percentile(means,[2.5,97.5])])


def sign_test(values):
    nonzero=[v for v in values if finite(v) and abs(v)>1e-12]
    n=len(nonzero);positive=sum(v>0 for v in nonzero);k=min(positive,n-positive)
    p=min(1.,2*sum(math.comb(n,i) for i in range(k+1))/2**n) if n else 1.
    return dict(n_nonzero_seeds=n,positive=positive,negative=n-positive,two_sided_exact_p=p)


def number(value):
    if value is None:return "未取得"
    if isinstance(value,bool):return "はい" if value else "いいえ"
    if finite(value):return f"{value:.4g}"
    return str(value).replace("|","/").replace("\n"," ")


def summary(stats):
    if not stats["n_seeds"]:return "未取得"
    ci=stats["ci95"]
    return f"{number(stats['mean'])} ± {number(stats['sd'])} / 中央値 {number(stats['median'])} / CI [{number(ci[0])}, {number(ci[1])}] (n={stats['n_seeds']})"


def lookup(value,path):
    for part in path.split("."):
        if not isinstance(value,dict):return None
        value=value.get(part)
    return value


def explicit_bool(data,paths):
    for path in paths:
        value=lookup(data,path)
        if isinstance(value,bool):return value
    return None


def status_for_contrast(contrast):
    stats=contrast["statistics"];mean=stats["mean"]
    if stats["n_seeds"]<8:return "PARTIAL"
    if mean is None:return "PARTIAL"
    p=contrast["sign_test"]["two_sided_exact_p"]
    if stats["ci95"][0]>0 and p<=.05:return "PASS"
    if stats["ci95"][1]<0 and p<=.05:return "FAIL"
    if all(abs(v)<=1e-12 for v in contrast["paired_differences"]):return "FAIL"
    return "PARTIAL"


def positive_contrast(left,right,key):
    by_a={r["seed"]:lookup(r.get("metrics",r),key) for r in left}
    by_b={r["seed"]:lookup(r.get("metrics",r),key) for r in right}
    seeds=sorted(s for s in by_a.keys()&by_b.keys() if finite(by_a[s]) and finite(by_b[s]))
    delta=[by_a[s]-by_b[s] for s in seeds]
    result=dict(seed_ids=seeds,paired_differences=delta,statistics=statistics(delta),sign_test=sign_test(delta))
    result["status"]=status_for_contrast(result)
    return result


def delta_text(contrast):
    return f"{summary(contrast['statistics'])}; exact sign p={number(contrast['sign_test']['two_sided_exact_p'])}; {contrast['status']}"


def table(headers,rows):
    return "\n".join(["| "+" | ".join(headers)+" |","|"+"|".join("---" for _ in headers)+"|"]+["| "+" | ".join(str(x).replace("|","/").replace("\n"," ") for x in row)+" |" for row in rows])


def save(path,value):
    temp=path.with_name(path.name+".tmp");temp.write_text(json.dumps(value,ensure_ascii=False,indent=2,allow_nan=False));temp.replace(path)


def generate(artifacts, strict=False):
    root=Path(artifacts);root.mkdir(parents=True,exist_ok=True)
    data={};missing=[];input_hashes={}
    for name in REQUIRED:
        path=root/name
        if not path.exists():data[name]=None;missing.append(name);continue
        raw=path.read_bytes();input_hashes[name]=hashlib.sha256(raw).hexdigest()
        try:data[name]=json.loads(raw)
        except ValueError:data[name]=None;missing.append(name+"（JSON破損）")
    for name in ["stabilized_selection.json","final_runtime_state.json","j72_health.json","resource_summary.json"]:
        optional=root/name
        if optional.exists():
            raw=optional.read_bytes();data[name]=json.loads(raw);input_hashes[name]=hashlib.sha256(raw).hexdigest()
    selection_stats={}
    for arch in ARCHES:
        values=[]
        for path in sorted((root/"runs").glob(arch+"-s*-B0/selection.json")):
            raw=path.read_bytes();record=json.loads(raw)
            input_hashes[str(path.relative_to(root))]=hashlib.sha256(raw).hexdigest()
            if record.get("validation_seed")==700001:values.append(record.get("score"))
        selection_stats[arch]=statistics(values)
    eligible={a:v for a,v in selection_stats.items() if v["n_seeds"]==8}
    best_arch=max(eligible,key=lambda a:eligible[a]["mean"]) if eligible else None
    selection_text="validation seed700001の選択score（scenario macro成功率 + 0.1×episode reward）の8 seed平均："+"、".join(f"{NAMES[a]} {format(selection_stats[a]['mean'],'.6f') if selection_stats[a]['mean'] is not None else '未取得'}" for a in ARCHES)+"。best Coreは "+(NAMES[best_arch] if best_arch else "未確定")+"。軽量候補GRU64は別に保持し、test最高seedで選び直さない。"
    def rows(name):return data.get(name) if isinstance(data.get(name),list) else []
    def obj(name):return data.get(name) if isinstance(data.get(name),dict) else {}
    main=rows("run_summary.json")
    baseline=[r for r in main if r.get("checkpoint_name")=="imitation_best.pt" and r.get("run_id","").endswith("-B0")]
    ppo=[r for r in main if r.get("checkpoint_name")=="ppo_final.pt" and r.get("architecture")=="gru128"]
    coverage={arch:sorted({r.get("seed") for r in baseline if r.get("architecture")==arch}) for arch in ARCHES}
    archstats={arch:{key:statistics([r.get(key) for r in baseline if r.get("architecture")==arch]) for key in METRICS} for arch in ARCHES}
    def select(name,arch,condition=None):
        return [r for r in rows(name) if r.get("architecture")==arch and (condition is None or r.get("condition")==condition)]
    contrasts={}
    for arch in ARCHES:
        contrasts[arch]={
            "learned_minus_never_language":positive_contrast(select("policy_interventions.json",arch,"learned"),select("policy_interventions.json",arch,"never"),"scenario_success.language"),
            "learned_minus_matched_language":positive_contrast(select("policy_interventions.json",arch,"learned"),select("policy_interventions.json",arch,"matched_rate_random"),"scenario_success.language"),
            "correct_minus_shuffled_language":positive_contrast(select("language_response_ablation.json",arch,"correct"),select("language_response_ablation.json",arch,"shuffled"),"scenario_success.language"),
            "correct_minus_missing_language":positive_contrast(select("language_response_ablation.json",arch,"correct"),select("language_response_ablation.json",arch,"missing"),"scenario_success.language"),
            "normal_minus_resetstep_delay40":positive_contrast([r for r in select("retention_results.json",arch) if r.get("delay")==40 and r.get("state_mode")=="normal"],[r for r in select("retention_results.json",arch) if r.get("delay")==40 and r.get("state_mode")=="reset_step"],"memory_retention")}
    memory_vs_mlp={arch:positive_contrast([r for r in select("retention_results.json",arch) if r.get("delay")==40 and r.get("state_mode")=="normal"],[r for r in select("retention_results.json","mlp") if r.get("delay")==40 and r.get("state_mode")=="normal"],"memory_retention") for arch in ["gru64","gru128"]}
    trained=obj("trained_delay_ablation.json")
    trained_rows=trained.get("records",rows("trained_delay_ablation.json"))
    def trained_select(arch,mode):
        return [r for r in trained_rows if r.get("architecture")==arch and r.get("delay")==47 and r.get("state_mode")==mode]
    trained_memory={a:positive_contrast(trained_select(a,"normal"),trained_select(a,"reset_step"),"memory_retention") for a in ARCHES}
    trained_vs_mlp={a:positive_contrast(trained_select(a,"normal"),trained_select("mlp","normal"),"memory_retention") for a in ["gru64","gru128"]}
    memory_pairs=trained.get("memory_pairs",[])
    pair_contrasts={a:positive_contrast([r for r in memory_pairs if r.get("architecture")==a],[r for r in memory_pairs if r.get("architecture")=="mlp"],"paired_success") for a in ["gru64","gru128"]}
    ppoarms=sorted({r.get("run_id","").split("-")[-1] for r in ppo})
    ppocontrasts={}
    for arm in ppoarms:
        left=[r for r in ppo if r["run_id"].endswith("-"+arm)]
        ppocontrasts[arm]={"vs_B0":positive_contrast(left,[r for r in baseline if r.get("architecture")=="gru128"],"task_success"),
                          "vs_B1":positive_contrast(left,[r for r in ppo if r["run_id"].endswith("-B1")],"task_success")}
    validation=obj("validation.json");evidence=obj("run_evidence.json");leak=obj("leakage_audit.json");j72=obj("j72_results.json")
    tests_passed=explicit_bool(validation,["passed","tests_passed","tests.passed","all_tests_passed"])
    if tests_passed is None:
        groups=[validation.get(k,{}) for k in ["unit_and_integration","gui_offscreen"]]
        if all(isinstance(g.get("passed"),int) and g.get("passed",0)>0 and isinstance(g.get("failed"),int) for g in groups):
            tests_passed=all(g["failed"]==0 for g in groups)
    auditrows=rows("ambiguity_audit.json")
    audit_complete=len({(r.get("architecture"),r.get("seed")) for r in auditrows})==24
    audit_pass=bool(auditrows) and all(r.get("metrics",{}).get("passed") is True for r in auditrows)
    same_forks=bool(rows("counterfactual.json")) and all(r.get("metrics",{}).get("identical_environment_state_at_fork") is True and r.get("metrics",{}).get("identical_model_state_at_fork") is True for r in rows("counterfactual.json"))
    matched_records=[r for r in rows("policy_interventions.json") if r.get("condition")=="matched_rate_random"]
    matched_exact=bool(matched_records) and all(r.get("metrics",{}).get("matched_accepted_calls_exact") is True for r in matched_records)
    parent_checked=explicit_bool(evidence,["ppo_parent_hashes_match","ppo_parent_checks_passed","ppo_pairing.parent_hashes_match","checks.ppo_parent_hashes_match"])
    if parent_checked is None and evidence.get("paired_checks"):
        checks=evidence["paired_checks"]
        parent_checked=sorted(p.get("seed") for p in checks)==list(range(8)) and all(all(p.get(k) is True for k in ["same_initial_model","same_parent_checkpoint","same_rollout_rng","same_gpu"]) for p in checks)
    all8=all(seeds==list(range(8)) for seeds in coverage.values())
    combined=obj("stabilized_selection.json")
    combined_required=bool(combined.get("factors"))
    combined_complete=not combined_required or sorted({r["seed"] for r in ppo if r["run_id"].endswith("-B8")})==list(range(8))
    ppo8=all(sorted({r["seed"] for r in ppo if r["run_id"].endswith("-"+arm)})==list(range(8)) for arm in [f"B{i}" for i in range(1,8)])
    def any_arch_status(key):
        statuses=[contrasts[arch][key]["status"] for arch in ["gru64","gru128"]]
        return "PASS" if "PASS" in statuses else ("FAIL" if all(s=="FAIL" for s in statuses) else "PARTIAL")
    gate={}
    def add(name,status,detail):gate[name]={"status":status,"evidence":detail}
    add("closed_loop_action_changes_future_observation","PASS" if tests_passed is True else ("FAIL" if tests_passed is False else "PARTIAL"),"validation.jsonのE2テスト結果。個別環境テストの範囲に限定し、実サービス動作とは区別する。")
    add("precall_ambiguity","PASS" if audit_complete and audit_pass else ("FAIL" if auditrows and not audit_pass else "PARTIAL"),f"対象{len(auditrows)} seed記録。no-call A/B履歴と行動のbit一致・balanced成功率≤0.5を照合。")
    add("returned_information_used",any_arch_status("correct_minus_shuffled_language"),"GRU64/GRU128のcorrect−shuffled差（言語subset）、8 seed CIとexact sign test。")
    add("learned_gate_better_than_never",any_arch_status("learned_minus_never_language"),"言語subsetでlearned−never、同一episode群。")
    add("learned_gate_better_than_matched_random",any_arch_status("learned_minus_matched_language") if matched_exact else "PARTIAL","受理call数一致の確認とlearned−global matched random差。episode割当と時点を両方変えるため、純粋なtiming効果ではない。")
    add("oracle_upper_bound","PARTIAL","情報制約付きanalytic参照との比較。大域最適性・厳密な上界は証明していない。oracle gate後の行動には学習policyを使用する。")
    leakage_ok=leak.get("seed_overlap") is False and leak.get("duplicated_exogenous_episodes")==[] and audit_complete and audit_pass and leak.get("baseline_unchanged") is True
    add("leakage_audit","PASS" if leakage_ok and tests_passed is True else ("FAIL" if leak.get("seed_overlap") is True or bool(leak.get("duplicated_exogenous_episodes")) or leak.get("baseline_unchanged") is False else "PARTIAL"),"RNG stream分離、有限fingerprint、no-call A/B、strict memory、固定正規化、baseline hashを確認。未試験入力全体への無漏洩証明ではない。")
    memory_status=["PASS" if trained_memory[a]["status"]=="PASS" and trained_vs_mlp[a]["status"]=="PASS" else ("FAIL" if trained_memory[a]["status"]=="FAIL" or trained_vs_mlp[a]["status"]=="FAIL" else "PARTIAL") for a in ["gru64","gru128"]]
    pair_status=trained.get("memory_pair_audit_passed")
    if pair_status is None and memory_pairs:
        pair_status=len(memory_pairs)==24 and all(r.get("postcue_bit_identical") is True for r in memory_pairs)
    add("recurrent_memory_dependency","PASS" if any(memory_status[i]=="PASS" and pair_contrasts[a]["status"]=="PASS" and all(r.get("final_action_change_rate",0)>0 for r in memory_pairs if r.get("architecture")==a) for i,a in enumerate(["gru64","gru128"])) and pair_status is True else ("FAIL" if all(x=="FAIL" for x in memory_status) or pair_status is False else "PARTIAL"),"既知の訓練delay=47におけるnormal−reset every step、GRU−MLP、cue A/Bの消失後観測bit一致・最終行動分岐の補足診断。未知delayへの転移とは分離する。")
    temporal_stats={a:{d:statistics([r.get("metrics",{}).get("memory_retention") for r in select("retention_results.json",a) if r.get("state_mode")=="normal" and r.get("delay")==d]) for d in [8,16,24,40,80,160,320,640]} for a in ARCHES}
    temporal_fail=all(any(v["n_seeds"]==8 and v["ci95"][1]<=.5 for v in temporal_stats[a].values()) for a in ["gru64","gru128"])
    add("memory_temporal_ood_generalization","FAIL" if temporal_fail else "PARTIAL","訓練47以外の事前指定8/16/24/40/80/160/320/640で評価。少なくとも一条件で両GRUの95% CI上限≤chance .5なら広い時刻転移はFAIL。非単調な破綻から単一memory horizonは定義しない。")
    add("ppo_causal_dissection","PASS" if ppo8 and parent_checked is True else "PARTIAL",f"B1–B7各8 seed={ppo8}。同一parent hash検証={parent_checked}。PPOが改善したこと自体を要求せず、退行を含めて因子別に報告する。")
    add("paired_causal_forks","PASS" if same_forks and len(rows("counterfactual.json"))==24 else "PARTIAL","同一env snapshotとmodel stateからCALL/NO CALL分岐。後続callを両群で無効化した単回強制call estimand。")
    representatives=j72.get("representatives",[])
    j72_closed=bool(representatives) and j72.get("complete") is True and all(r.get("calls",0)>0 and r.get("api_success")==1 and r.get("parse_success")==1 and r.get("semantic_correctness")==1 and r.get("finite_core_state") is True and r.get("downstream_success",0)>0 for r in representatives)
    add("j72_closed_loop","PASS" if j72_closed else ("FAIL" if representatives and j72.get("complete") is True and all(r.get("calls",0)==0 or r.get("semantic_correctness")==0 for r in representatives) else "PARTIAL"),"学習gateによるlive呼び出し、HTTP、strict parse、意味正解、Core downstreamを分離。forced-gate補助診断は学習gateの成功に含めない。")
    expected_conditions={"voi_results.json":19,"policy_interventions.json":6,"language_response_ablation.json":9,"counterfactual.json":1,"ambiguity_audit.json":1,"active_sensing.json":10,"retention_results.json":40,"ood_results.json":14,"habituation_results.json":12}
    category_coverage={}
    for filename,expected in expected_conditions.items():
        groups={(a,seed):[r for r in rows(filename) if r.get("architecture")==a and r.get("seed")==seed] for a in ARCHES for seed in range(8)}
        complete=all(len(group)==expected and len({r.get("condition") for r in group})==expected for group in groups.values())
        category_coverage[filename]={"expected_conditions_per_seed":expected,"record_count":len(rows(filename)),"complete":complete}
    plot_missing=[p for p in PLOTS if not (root/p).is_file() or (root/p).stat().st_size<128]
    trained_complete=trained.get("complete") is True and len(trained_rows)==120 and len(memory_pairs)==24
    runtime=obj("final_runtime_state.json")
    runtime_safe=runtime.get("restored_stopped_state") is True and runtime.get("training_workers_terminated") is True and runtime.get("gpu_compute_processes")==[]
    native_pass=lookup(validation,"native_gui.status")=="PASS"
    execution=native_pass and runtime_safe and parent_checked is True and trained_complete and all8 and ppo8 and combined_complete and all(v["complete"] for v in category_coverage.values()) and not missing and not plot_missing and j72.get("complete") is True and tests_passed is True
    research="FAIL" if any(v["status"]=="FAIL" for v in gate.values()) else ("PASS" if all(v["status"]=="PASS" for v in gate.values()) else "PARTIAL")
    success={"execution_complete":execution,"execution_scope":"Saved experiment matrix, evaluation, tests, native GUI, parent/RNG pairing and stopped runtime evidence only. Final docs/commit completeness requires root final audit.","runtime_restored":runtime_safe,"native_gui_passed":native_pass,"research_status":research,"architecture_seed_coverage":coverage,"expected_seeds":list(range(8)),"ppo_B1_B7_eight_seed_coverage":ppo8,"combined_B8_required":combined_required,"combined_B8_complete":combined_complete,"trained_memory_supplement_complete":trained_complete,"category_coverage":category_coverage,"missing_inputs":missing,"missing_plots":plot_missing,"conditions":gate,
             "decision_rule":"PASS requires >=8 independent paired seeds, positive difference CI lower>0 and exact two-sided sign p<=.05 for directional behavioral claims; negative CI upper<0 and exact p<=.05, or all-zero paired differences, is FAIL; inconclusive and incomplete evidence is PARTIAL. Exploratory multiple comparisons are not familywise corrected.","input_sha256":input_hashes}
    voi_contrasts={}
    for arch in ARCHES:
        voi_contrasts[arch]={}
        for dimension,low,high in [("cost",0,1.2),("reliability",1.,.5),("latency",0,16)]:
            selected=[r for r in select("voi_results.json",arch) if r.get("dimension")==dimension]
            voi_contrasts[arch][dimension]=positive_contrast([r for r in selected if r.get("value")==low],[r for r in selected if r.get("value")==high],"llm_call_rate")
    success["strong_call_responsiveness"]={arch:{dimension:value["status"] for dimension,value in tests.items()} for arch,tests in voi_contrasts.items()}
    success["strong_condition_limit"]="These test call-frequency response only. Rational performance preservation and reliability-dependent reliance require joint interpretation, not call-rate alone."
    output=[]
    def section(title,text):output.extend(["## "+title,"",text,""])
    output.extend(["# K0-E2 Active Information Acquisition 実験報告","",f"実行完了：**{'完了' if execution else '未完了／証拠確認待ち'}**。研究成功条件：**{research}**。",""])
    if missing or plot_missing:output.extend(["未取得："+"、".join(missing+plot_missing)+"。以下では欠測をゼロや成功へ置き換えない。",""])
    findings=[]
    for arch in ["gru64","gru128"]:
        findings.append(f"{NAMES[arch]}：returned informationの利用（correct−shuffled、言語subset）{delta_text(contrasts[arch]['correct_minus_shuffled_language'])}。")
    findings.append(f"GRU128の訓練時刻での記憶依存（delay=47、normal−reset every step）{delta_text(trained_memory['gru128'])}。未知時刻への記憶転移：{gate['memory_temporal_ood_generalization']['status']}。")
    if "B1" in ppocontrasts:findings.append("現行PPO B1−B0成功率差："+delta_text(ppocontrasts["B1"]["vs_B0"])+"。負値は退行であり改善扱いしない。")
    findings.append("cost 0→1.2のcall率低下："+"、".join(f"{NAMES[a]} {voi_contrasts[a]['cost']['status']}（平均低下 {number(voi_contrasts[a]['cost']['statistics']['mean'])}）" for a in ["gru64","gru128"])+"。高い成功率と合理的な取得cost判断は別の課題である。")
    findings.append("J72の実API・parse・意味情報・Core閉ループ判定："+gate["j72_closed_loop"]["status"]+"。HTTP成功だけで言語器官の成功としない。")
    findings.append(selection_text)
    section("1. 結論","\n\n".join(findings))
    section("2. 研究質問と仮説","H1：現在の観測だけで解けない記憶。H2：ORIENT/OBSERVEによる能動観測。H3：言語器官を情報取得に利用。H4：cost・reliability・latencyへの条件依存。H5：戻った情報が後続行動と成功を変える。H6：PPO退行の因子切り分け。相関、call率、HTTP 200だけを成功根拠にしない。")
    section("3. K0-Eとの差分","K0-Eのcheckpoint・集約成果物をimmutable baselineとして保全した独立namespaceである。K0-E2では行動が未来観測を変え、CALL→応答→固定長数値interface→Core→次行動を閉じる。K0は常時稼働する非言語controller、J72は条件付きで参照する言語器官と位置付ける。Git push・外部公開は行わない。baseline hashの確認結果は後述する。")
    section("4. Environment設計","16次元float観測、6行動、同期した48-step部分観測環境。言語A/B pairは非言語行動だけでは履歴が一致し、最終正解はOBSERVE/RECALLで異なる。正解latentとscenario IDはpolicy入力にしない。一方、合成的な物理affordanceから必要な取得操作の種類を識別できるため、未知task type推論の証明ではない。deadline flagとservice条件は公開する。strict-memoryではcue消失後の外部RECALL再提示とcue依存の環境書込みを抑制する。")
    section("5. 情報取得設計","ORIENTは短期SNR改善、OBSERVEは物理的追加情報、RECALLは過去に実際に観測した情報の再提示を意味する。strict-memoryでは消えたcueを再取得できない。言語callは1 episodeに1回受理しcostを課し、遅延後のcategory/confidence情報を観測へ戻す。callした瞬間には成功しない。language latency=0はcallの直後の次観測が最速で、追加L stepなら到着までL+1遷移。言語応答はdecisionまで持続表示されるため、CALL後にMLPが解けても漏洩とは限らない。")
    configs=[]
    for path in sorted((root/"runs").glob("*-B0/config.json")):
        try:configs.append(json.loads(path.read_text()))
        except ValueError:pass
    train_rows=[]
    for key,label in [("imitation_updates","imitation updates"),("num_envs","並列環境"),("ppo_updates","PPO updates")]:
        values=sorted({c[key] for c in configs if key in c});train_rows.append([label,", ".join(map(str,values)) if values else "config未取得"])
    section("6. 訓練条件とprotocol変更",table(["項目","保存config値"],train_rows)+"\n\nprimaryは3構成×8独立seed、256 imitation更新（前半128 teacher、後半128はteacher 80%/learner 20% DAgger exposure）。64更新pilotは別保存し、held-outを見る前のvalidation学習不足を根拠に共通のfresh protocolを1度だけ追加した。teacher-only期間とDAgger期間も変わるので、64→256差を純粋な計算予算の因果効果として解釈しない。1024並列環境、48 step、全sequence勾配、learning rate .001、決定step重み32・取得行動重み4を用いる。validation seed=700001、held-out=900001。評価時の基本条件はcost=.05、reliability=1、追加latency=2。")
    stat_table=[]
    for key in ["task_success","scenario_macro_success","reward","oracle_regret","llm_call_rate","required_call_recall","required_call_precision","required_call_f1","false_call_rate","missed_required_rate","action_macro_f1","memory_retention","habituation_sequence_success","novelty_reaction","params","cpu_inference_ms_median","cpu_inference_ms_p95","training_steps_per_second_mean"]:
        stat_table.append([METRICS[key]]+[summary(archstats[a][key]) for a in ARCHES])
    scenario_rows=[];action_rows=[]
    for arch in ARCHES:
        selected=[r for r in baseline if r.get("architecture")==arch]
        for scenario,label in SCENARIOS.items():
            scenario_rows.append([NAMES[arch],label,summary(statistics([lookup(r,"scenario_success."+scenario) for r in selected]))])
        for index,action in enumerate(["IGNORE","WAIT","ORIENT","OBSERVE","RECALL","INVOKE_LANGUAGE"]):
            action_rows.append([NAMES[arch],action]+[summary(statistics([r.get(key,[])[index] if len(r.get(key,[]))>index else None for r in selected])) for key in ["per_action_precision","per_action_recall","per_action_f1"]])
    section("7. Architecture比較",table(["指標：mean±SD / median / 95% CI",*map(NAMES.get,ARCHES)],stat_table)+"\n\n"+table(["構成","シナリオ","成功率"],scenario_rows)+"\n\n"+table(["構成","行動","precision","recall","F1"],action_rows)+"\n\n混同行列は正解行×予測列、行動順 IGNORE/WAIT/ORIENT/OBSERVE/RECALL/INVOKE_LANGUAGE で raw run_summary.json に保存。集計はseedごとの各指標を等重みとする。\n\n全scalar指標の再計算値は [report_statistics.json](report_statistics.json)。meanは独立seedを等重みとし、episodeを独立反復にしない。oracle regretは情報制約付きanalytic参照reward−learned rewardであり、参照policyの大域最適性は証明していない。")
    mem_rows=[]
    for arch in ARCHES:
        normal=trained_select(arch,"normal")
        mem_rows.append([NAMES[arch],summary(statistics([r["metrics"].get("memory_retention") for r in normal])),delta_text(trained_memory[arch]),delta_text(trained_vs_mlp[arch]) if arch in trained_vs_mlp else "memoryless control"])
    pair_rows=[]
    for a in ARCHES:
        selected=[r for r in memory_pairs if r.get("architecture")==a]
        pair_rows.append([NAMES[a],str(sum(r.get("postcue_bit_identical") is True for r in selected))+"/"+str(len(selected)),summary(statistics([r.get("paired_success") for r in selected])),summary(statistics([r.get("final_action_change_rate") for r in selected])),delta_text(pair_contrasts[a]) if a in pair_contrasts else "memoryless control"])
    temporal_rows=[[NAMES[a],str(d),summary(st)] for a in ARCHES for d,st in temporal_stats[a].items()]
    section("8. Memory結果",table(["構成","訓練delay47 normal","normal−reset every step","GRU normal−MLP normal"],mem_rows)+"\n\nこの追加診断は既知の訓練時刻47のcontrol不足を補うもので、好成績のtest delayを探索して選んだものではない。primary評価とcheckpointを保全し、学習・model選択には使わない。cue A/Bが消えた後の観測bit一致とnormal policyの最終行動分岐を同時に確認する。 [trained_delay_ablation.json](trained_delay_ablation.json)\n\n"+table(["構成","cue消失後bit一致seed","balanced A/B成功率","最終行動変化率","GRU−MLP paired成功率"],pair_rows)+"\n\n"+table(["構成","未知のdelay","normal記憶成功率"],temporal_rows)+"\n\n事前指定8/16/24/40/80/160/320/640 stepではnormal/reset each step/reset8/noise/quantizeを比較する。訓練は47であり、これらは記憶長だけでなく決定時刻OODを含む。未知時刻での非単調な破綻は時刻generalizationの失敗として残し、単一memory horizonや50% crossingは定義しない。各条件64 episodeが標準だが、実数は各JSONのepisodesに従う。 [retention_curve.png](retention_curve.png)")
    sensing_rows=[]
    for arch in ARCHES:
        for acquisition in ["orient","observe"]:
            contrast=positive_contrast(select("active_sensing.json",arch,acquisition+"/learned"),select("active_sensing.json",arch,acquisition+"/disabled"),"task_success")
            sensing_rows.append([NAMES[arch],acquisition.upper(),delta_text(contrast)])
    section("9. Active sensingと慣れ",table(["構成","取得操作","learned−disabled成功率"],sensing_rows)+"\n\nforced/disabled/oracle/random/learnedを同一seedで評価。強制は開始時に1回の取得操作を与える介入で、あらゆる時点に強制する操作ではない。habituationはfirst ORIENT→repeat IGNORE→novel ORIENTを要求し、novelty強度・反復回数・gap・noiseの有限sweepを保存する。シナリオのaffordanceと既定のevent構造への依存は残る。 [active_sensing.json](active_sensing.json) / [habituation_results.json](habituation_results.json)")
    voi_rows=[]
    for arch in ARCHES:
        for dimension,metric,low,high in [("cost","llm_call_rate",0,1.2),("reliability","llm_call_rate",1.,.5),("latency","llm_call_rate",0,16)]:
            selected=[r for r in select("voi_results.json",arch) if r.get("dimension")==dimension]
            contrast=positive_contrast([r for r in selected if r.get("value")==low],[r for r in selected if r.get("value")==high],metric)
            voi_rows.append([NAMES[arch],f"{dimension}: {low}−{high}",delta_text(contrast)])
    section("10. Language VoI",table(["構成","比較（call率差）","seed対応結果"],voi_rows)+"\n\ncost基本grid=0/.01/.02/.05/.10/.20/.40と、±1報酬のanalytic閾値を横断する事前補足=.8/1.2を分けて扱う。後者をtest結果を見た追加tuningとは扱わない。reliability=1/.9/.75/.5、追加latency=0/1/2/4/8/16の一因子sweepである。上表で正差ならcost上昇・信頼度低下・latency増加に伴うcall減少。ただし成功率やrewardを保つかは別指標であり、call減少のみを合理的VoIとしない。 [voi_cost_curve.png](voi_cost_curve.png) / [voi_performance_curve.png](voi_performance_curve.png) / [reliability_curve.png](reliability_curve.png) / [latency_curve.png](latency_curve.png)")
    causal_rows=[]
    for arch in ARCHES:
        selected=select("counterfactual.json",arch)
        causal_rows.append([NAMES[arch]]+[summary(statistics([r.get("metrics",{}).get(k) for r in selected])) for k in ["delta_reward","delta_success","delta_latency_wait_steps","delta_downstream_action_rate"]])
    response_rows=[]
    for arch in ARCHES:response_rows.append([NAMES[arch]]+[delta_text(contrasts[arch][k]) for k in ["correct_minus_shuffled_language","learned_minus_never_language","learned_minus_matched_language"]])
    section("11. Counterfactualとreturned information",table(["構成","CALL−NO CALL reward","成功率差","WAIT step差","後続行動の不一致率"],causal_rows)+"\n\nstep1で同一env・model stateからforkし、最初のCALL有無を変え、その後のcallは両群で無効化する。これは単回強制callのcausal estimandで、自然に学習されたgateの平均処置効果ではない。エピソードdeadlineは固定なので総episode長差は0。WAIT回数差、サービス情報到着遅延、HTTP秒を混同しない。\n\n"+table(["構成","correct−shuffled：言語subset","learned−never：言語subset","learned−matched random：言語subset"],response_rows)+"\n\nmatched randomはglobal episode割当と時点を再配置し、受理call数を正確に一致させる。タイミング単独の効果ではない。応答shuffled等の介入は入力情報利用の証拠であり、時系列全体shuffleのoffline診断とは区別する。 [policy_intervention.png](policy_intervention.png) / [response_ablation.png](response_ablation.png)")
    ppo_rows=[]
    for arm in ppoarms:
        selected=[r for r in ppo if r["run_id"].endswith("-"+arm)]
        ppo_rows.append([arm,summary(statistics([r.get("task_success") for r in selected])),("観測上は変化なし・性能保持（優越性なし、一般的非劣性の証明ではない）; "+summary(ppocontrasts[arm]["vs_B0"]["statistics"]) if all(abs(v)<1e-12 for v in ppocontrasts[arm]["vs_B0"]["paired_differences"]) and ppocontrasts[arm]["vs_B0"]["statistics"]["n_seeds"]==8 else delta_text(ppocontrasts[arm]["vs_B0"])),delta_text(ppocontrasts[arm]["vs_B1"]) if arm!="B1" else "基準"])
    stabilization_rows=[[r.get("arm"),summary(r.get("score",statistics([]))),summary(r.get("delta_vs_B1",statistics([]))),number(lookup(r,"sign_test.two_sided_exact_p")),number(r.get("eligible"))] for r in combined.get("rows",[])]
    stabilization_text="\n\n複合候補のvalidation限定選択（testは不使用）："+", ".join(combined.get("factors",[]))+"。\n\n"+table(["因子","final validation score","B1との差","exact sign p","採用候補"],stabilization_rows) if combined else "\n\n複合候補のvalidation選択記録は未取得。"
    section("12. PPO ablation",table(["arm","PPO-final成功率","arm−B0 imitation-best","arm−B1"],ppo_rows)+"\n\nB1現行PPO、B2言語cost reward項のみゼロ、B3低LR .0001、B4 critic headのみ4更新warm-up、B5 return SDによるvalue loss scale、B6 imitation KL anchor .1、B7強いgradient clip .25。B1は既にadvantage normalizationとclip1.0を持つため、B5/B7を機能の有無比較とは呼ばない。各seedは同じimitation-best parent/optimizerと共通rollout RNGから始める。B4の追加computeは他因子と別記する。単一変更以外の交絡をparent hash/RNG証跡で確認する。複合候補B8等が存在すれば表へ自動追加するが、validationのみで選んだことの証拠が必要。改善がなくてもimitationを保持し、PPO成功としない。 [ppo_ablation.png](ppo_ablation.png)"+stabilization_text)
    ood_rows=[];worst={}
    for arch in ARCHES:
        grouped={}
        for condition in sorted({r.get("condition") for r in select("ood_results.json",arch)}):
            stats=statistics([r.get("metrics",{}).get("task_success") for r in select("ood_results.json",arch,condition)])
            grouped[condition]=stats;ood_rows.append([NAMES[arch],condition,summary(stats)])
        valid={k:v for k,v in grouped.items() if v["mean"] is not None}
        worst[arch]=min(valid,key=lambda k:valid[k]["mean"]) if valid else None
    section("13. OOD",table(["構成","named perturbation","成功率"],ood_rows)+"\n\n最弱の個別条件："+"、".join(f"{NAMES[a]}={worst[a] or '未取得'}" for a in ARCHES)+"。14 named conditionsだがsensor_failure/constant_sensor/stuck_atなどは同じmechanismを含む。重複列を独立な摂動と数えた単純平均を頑健性指標には使わない。response inverted/plausible_wrong、missing/contradictoryもCore観測が等価になり得る。 [ood_heatmap.png](ood_heatmap.png)")
    j72_rows=[];fault_rows=[]
    for item in representatives:
        j72_rows.append([item.get("run_id","未取得"),number(item.get("calls")),number(item.get("api_success")),number(item.get("parse_success")),number(item.get("semantic_correctness")),number(item.get("downstream_success")),number(lookup(item,"scripted.downstream_success")),number(item.get("response_latency_seconds"))])
        for kind,result in item.get("faults",{}).items():fault_rows.append([item.get("run_id"),kind,number(result.get("finite_core_state")),number(result.get("downstream_success")),"controlled fixture"])
    health=obj("j72_health.json")
    health_text="health応答のAPI label="+number(health.get("model"))+"、実parameter総数="+str(lookup(health,"params.total") or "未取得")+"。API label中の30mを実parameter数と読み替えない。"
    section("14. J72 end-to-end",health_text+"\n\n"+table(["validation選択代表","live calls","API成功","parse成功","意味正解","Core成功","scripted Core成功","応答平均秒"],j72_rows)+"\n\n実行層：HTTP→strict JSON parser→category/confidence/evidence_id→数値観測→Core hidden state→次行動。語彙的なA/B record抽出課題であり一般推論や自然言語理解を証明しない。J72代表はvalidationで選び、held-outで選び直さない。forced_gate_diagnosticがある場合、それはinterface診断であり学習gate成功へ合算しない。\n\n"+table(["代表","異常条件","finite Core","downstream成功","証拠範囲"],fault_rows)+"\n\ntimeout/HTTP503/malformed/empty/contradiction/slowは決定論的transport fixtureであり、実サービスで同じ故障を起こした証明ではない。HTTPの実wall-clock秒と環境の固定step latencyも異なる。サービスを一時起動し元の状態へ復元した証拠はfinal_runtime_state.jsonを参照する。")
    failures=[f"{name}: {value['status']} — {value['evidence']}" for name,value in gate.items() if value["status"]!="PASS"]
    section("15. 失敗例と未達", "\n\n".join(failures) if failures else "記録した判定では未達なし。ただし有限の合成環境に限定した結果である。")
    section("16. 統計","独立単位はtraining seedであり、3構成各8 seedを基本とする。episodeとstepはseed内の測定で、独立nやp値を増やさない。各scalarについてmean、sample SD、median、4000回のseed bootstrap percentile 95% CIを計算。paired比較は同じseedと同じ評価episode RNGを対応させ、zero差を除いたexact two-sided sign testを示す。CIは訓練seed変動だけを表し、全seedが共有する固定held-out環境からの母集団sampling不確実性を含まない。幅0のCIも確実性や一般化を保証しない。小標本のCIとp値を併記し、多数の副次比較はfamilywise補正をしていない探索的結果として扱う。主要behavioral PASSはn≥8、平均差正、CI下限>0、sign p≤.05とする。これは効果の一般性を保証しない。詳細な対応差は [report_statistics.json](report_statistics.json)。")
    section("17. 限界","合成binary fact、公開service条件、既知の物理affordance、固定deadline、1 episode 1call、decisionまで残る応答という制限がある。厳密なoracle最適性は未証明。time shuffleはfrozen baseline sensor tapeの非因果診断で、未来情報を前倒しし得るためH1/H3の因果根拠に使わない。個別maskには公開protocol channelの破壊が含まれ、そこでの性能低下をshortcut発見と即断しない。logical active connection数はcompute proxyであり、physical sparse speedupやFLOPs測定ではない。64-update pilotとprimaryは同一seedでも独立反復としてpoolしない。J72はA/B抽出のみであり大規模推論の品質は未評価。")
    section("18. 成功条件判定",table(["条件","判定","根拠"],[[k,v["status"],v["evidence"]] for k,v in gate.items()])+f"\n\n実行完了={execution}、研究成功={research}。結果は [success_criteria.json](success_criteria.json) に機械可読で保存する。")
    section("19. 次phase判断",selection_text+"\n\nこのデータからK1進行を自動承認しない。returned information利用・never/matched random優位・memory依存・漏洩監査の不足があれば、その具体的な環境／学習問題を先に解消する。GRU64とGRU128がvalidationで同等なら軽量なGRU64を優先し、GRU128だけが明確に優れるならGRU128をbaselineとする。単一seedのtest最高値をbest Coreとは呼ばない。最終ユーザー確認は全実験・検証・commit後に行う。")
    section("20. 再現方法","保存されたsource manifest・checkpoint hash・config・RNG状態・package一覧を揃え、同じprotocol identityで実行する。評価identityが異なる再実行は新artifact namespaceへ保存する。\n\n```bash\npython -m experiments.k0_e2_active_info.evaluate --artifacts experiments/k0_e2_active_info/artifacts/primary --all --num-envs 256 --long-envs 64 --threads 4\nfor cp in experiments/k0_e2_active_info/artifacts/primary/runs/*-B0/imitation_final.pt experiments/k0_e2_active_info/artifacts/primary/runs/*-B8/ppo_final.pt; do\n  python -m experiments.k0_e2_active_info.evaluate --artifacts experiments/k0_e2_active_info/artifacts/primary --checkpoint \"$cp\" --basic-only --threads 4\ndone\npython -m experiments.k0_e2_active_info.trained_delay --artifacts experiments/k0_e2_active_info/artifacts/primary\npython -m experiments.k0_e2_active_info.visualize --artifacts experiments/k0_e2_active_info/artifacts/primary\npython -m experiments.k0_e2_active_info.report --artifacts experiments/k0_e2_active_info/artifacts/primary\n```\n\n--allはB0-best全評価とB1–B7-final basic評価を実行する。別loopでimitation-finalとB8-finalを補完し、trained_delayで既知時刻47の補助診断を再現する。再評価は保存checkpointを使い、既存K0-Eを変更しない。required plot："+"、".join(f"[{p}]({p})" for p in PLOTS)+"。")
    hosts={r.get("cpu_benchmark_host") for r in baseline if r.get("cpu_benchmark_host")};threads={r.get("cpu_benchmark_threads") for r in baseline if r.get("cpu_benchmark_threads") is not None}
    total_tests=sum(validation.get(k,{}).get("passed",0) for k in ["unit_and_integration","gui_offscreen"])
    evidence_rows=[["一時service元の停止状態へ復元",number(runtime_safe)],["GPU compute process数",number(len(runtime["gpu_compute_processes"]) if isinstance(runtime.get("gpu_compute_processes"),list) else None)],["完了run数",number(evidence.get("run_count"))],["非finite checkpoint数",number(len(evidence["nonfinite_checkpoints"]) if isinstance(evidence.get("nonfinite_checkpoints"),list) else None)],["学習transition総数",str(evidence.get("total_training_transitions","未取得"))],["critic追加transition",str(evidence.get("additional_critic_transitions","未取得"))],["テストPASS数",str(total_tests)],["追加subtest PASS数",number(lookup(validation,"unit_and_integration.subtests_passed"))],["native GUI",number(lookup(validation,"native_gui.status"))],["baseline hash不変",number(leak.get("baseline_unchanged"))],["baseline対象file数",number(leak.get("baseline_files_checked"))],["seed stream重複",number(leak.get("seed_overlap"))],["重複したexogenous episode件数",number(len(leak["duplicated_exogenous_episodes"]) if isinstance(leak.get("duplicated_exogenous_episodes"),list) else None)],["CPU benchmark共通host数",number(len(hosts)) if hosts else "未取得"],["CPU thread設定",", ".join(map(str,sorted(threads))) if threads else "未取得"],["E2 tests PASS",number(tests_passed)],["PPO parent hash確認",number(parent_checked)]]
    section("21. 実行証跡と計算資源",table(["項目","保存値"],evidence_rows)+"\n\nRTX3060/P100は別workerで、同じseedは全armで同じ物理GPUを使う。GPU utilization・VRAMの生測定はgpu_telemetry.jsonl、集計はresource_summary.jsonを参照する。training wall timeとcheckpoint情報はrun_evidence.jsonに保存する。CPU推論は同一host・batch1・4threadsでmean/median/p95を区別する。PyTorch allocated memoryをnvidia-smiの総VRAM使用量と呼ばない。GPU worker終了、一時J72 service復元、commit一覧は最終root監査の証跡で確認する。このreport generatorはネットワーク呼び出し、学習、service変更、commit/pushを行わない。")
    report_text="\n".join(output)
    (root/"K0_E2_REPORT.md").write_text(report_text)
    save(root/"success_criteria.json",success)
    save(root/"success_conditions.json",success)
    save(root/"report_statistics.json",{"architecture":archstats,"validation_selection":selection_stats,"selected_architecture":best_arch,"paired_primary":contrasts,"memory_vs_mlp_delay40":memory_vs_mlp,"trained_memory_delay47":trained_memory,"trained_memory_vs_mlp_delay47":trained_vs_mlp,"trained_memory_cue_pairs":pair_contrasts,"memory_temporal_ood":temporal_stats,"ppo":ppocontrasts,"voi_call_responsiveness":voi_contrasts,"input_sha256":input_hashes})
    if strict and not execution:raise RuntimeError("Report generated with incomplete evidence; inspect success_criteria.json")
    return success


if __name__=="__main__":
    parser=argparse.ArgumentParser(description="保存されたK0-E2結果から日本語報告を生成")
    parser.add_argument("--artifacts",required=True);parser.add_argument("--strict",action="store_true")
    args=parser.parse_args();result=generate(args.artifacts,args.strict)
    print(json.dumps({"execution_complete":result["execution_complete"],"research_status":result["research_status"],"missing_inputs":result["missing_inputs"]},ensure_ascii=False,indent=2))
