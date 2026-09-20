"""Evidence-only Japanese K0-F2 report; failed probe never opens downstream data."""
from __future__ import annotations
import argparse
import hashlib
import json
import math
import statistics
from pathlib import Path

SECTIONS = (
    '結論','K0-Fからの変更点','confirmatory protocol lock','fresh data collection','sensor statistics',
    'InteroceptiveFrame','action-conditioned probe','probe validation gate','paired policy training',
    'BODY vs trained BLIND','BODY vs SHUFFLED','BODY vs STALE','counterfactual body','live hardware evaluation',
    'Mac contribution','temporal OOD','sensor OOD','leakage audit','statistics','safety','limitations',
    'success criteria','interpretation','next phase','reproducibility','runtime cleanup','commit history')
PLOTS = ('body_timeseries.png','action_conditioned_probe.png','probe_regret.png','body_vs_blind.png',
         'body_vs_shuffled.png','body_vs_stale.png','live_body_vs_blind.png','counterfactual_body.png',
         'sensor_ood_heatmap.png','temporal_ood.png','policy_action_map.png','pareto.png')
NOT_RUN = 'NOT_RUN_PROBE_GATE_FAILED'


def finite(value):
    return isinstance(value,(int,float)) and not isinstance(value,bool) and math.isfinite(value)


def mapping(value): return value if isinstance(value,dict) else {}
def rows(value): return value if isinstance(value,list) else mapping(value).get('rows',[])
def mean(records,key):
    values=[r[key] for r in records if finite(r.get(key))]
    return statistics.mean(values) if values else None


def fmt(value):
    if value is None:return '未取得 / 未評価'
    if isinstance(value,bool):return 'PASS' if value else 'FAIL'
    if finite(value):return f'{value:.6g}'
    if isinstance(value,float):return '無効値'
    if isinstance(value,(dict,list)):return json.dumps(value,ensure_ascii=False,sort_keys=True,separators=(',',':'))
    return str(value).replace('|','\\|').replace('\n',' ')


def table(headers,records):
    data=list(records)
    if not data:return '未実施 / 未評価。成功値は補完しない。'
    return '\n'.join(['| '+' | '.join(headers)+' |','| '+' | '.join('---' for _ in headers)+' |']+
                     ['| '+' | '.join(fmt(v) for v in row)+' |' for row in data])


class Evidence:
    def __init__(self,root): self.root=Path(root);self.inputs={};self.errors=[];self.opened=[]
    def read(self,name):
        path=self.root/name
        if not path.exists():return None
        self.opened.append(name)
        try:
            raw=path.read_bytes()
            def reject(value):raise ValueError('Nonfinite JSON')
            decode=lambda raw:json.loads(raw,parse_constant=reject)
            data=[decode(s) for s in raw.splitlines() if s.strip()] if path.suffix=='.jsonl' else decode(raw)
            self.inputs[name]=hashlib.sha256(raw).hexdigest()
            return data
        except (OSError,ValueError):self.errors.append(name+': 読み取り失敗');return None


def criterion(value, minimum_effect=.01):
    r=mapping(value);ci=r.get('ci95');delta=r.get('mean_difference',r.get('mean'));p=r.get('exact_sign_p')
    return bool(r.get('pass') is True and r.get('n_seeds')==12 and finite(delta) and delta>=minimum_effect
                and delta>0 and isinstance(ci,list) and len(ci)==2 and all(finite(x) for x in ci)
                and 0<ci[0]<=ci[1] and finite(p) and 0<=p<=.05)


def probe_numerical_gate(probe):
    """Two locked targets and selection regret must improve versus both controls."""
    data=rows(probe);selection=probe.get('selection_rows',[]);checks={}
    for target in ('log_latency','utility'):
        groups={m:[r for r in data if r.get('target')==target and r.get('mode')==m and r.get('split')=='validation'] for m in ('BODY','BLIND','SHUFFLED')}
        valid=all(len(v)==1 and finite(v[0].get('mae')) and v[0]['mae']>=0 for v in groups.values())
        for control in ('BLIND','SHUFFLED'):
            checks[f'{target}_BODY_better_{control}']=bool(valid and groups['BODY'][0]['mae']<groups[control][0]['mae'])
    groups={m:[r for r in selection if r.get('mode')==m] for m in ('BODY','BLIND','SHUFFLED')}
    valid=all(len(v)==1 and finite(v[0].get('selection_regret')) for v in groups.values())
    for control in ('BLIND','SHUFFLED'):
        checks[f'regret_BODY_better_{control}']=bool(valid and groups['BODY'][0]['selection_regret']<groups[control][0]['selection_regret'])
    checks['saved_gate']=mapping(probe.get('gate')).get('pass') is True
    return all(checks.values()),checks


def paired_rows(records,left,right):
    groups=[]
    for mode in (left,right):
        selected=[r for r in records if r.get('architecture','GRU128')=='GRU128' and display_mode(r)==mode]
        seeds=[r.get('seed') for r in selected]
        if sorted(seeds,key=lambda x: str(x)) != sorted(range(12),key=lambda x:str(x)) or len(seeds)!=12:return False
        if not all(finite(r.get('utility')) for r in selected):return False
        groups.append(set(seeds))
    return groups[0]==groups[1]


def display_mode(row):
    if row.get('training_mode')=='BLIND':return 'TRAINED_BLIND'
    return row.get('mode','未取得')


def metric_table(data):
    groups={}
    for r in rows(data):groups.setdefault(display_mode(r),[]).append(r)
    return table(('条件','独立seed n','効用','latency 秒','締切内成功率','失敗率'),
                 ((mode,len({r.get('seed') for r in group}),mean(group,'utility'),mean(group,'latency_seconds'),
                   mean(group,'deadline_success_rate'),mean(group,'failure_rate')) for mode,group in sorted(groups.items())))


def comparison_table(comparisons,keys,minimum_effect=.01):
    result=[]
    for key in keys:
        r=mapping(comparisons.get(key));ci=r.get('ci95')
        result.append((key,r.get('n_seeds'),r.get('mean_difference',r.get('mean')),r.get('sd'),r.get('median'),
                       '['+', '.join(fmt(x) for x in ci)+']' if isinstance(ci,list) else None,r.get('exact_sign_p'),
                       'PASS' if criterion(r,minimum_effect) else '未評価' if not r else 'FAIL'))
    return table(('比較','seed n','平均差','標本SD','中央値','95% CI','exact sign p','数値判定'),result)


def raw_summary(data):
    data=rows(data);ts=[r.get('timestamp') for r in data]
    valid=bool(data) and all(finite(t) for t in ts) and all(a<b for a,b in zip(ts,ts[1:]))
    return dict(samples=len(data),span_seconds=ts[-1]-ts[0] if valid else None,
                max_interval_seconds=max((b-a for a,b in zip(ts,ts[1:])),default=None) if valid else None,
                strictly_increasing=valid,source_kinds=sorted({r.get('source_kind','unknown') for r in data}))


def sensor_table(data,keys):
    result=[]
    for key in keys:
        values=[r.get('metrics',{}).get(key) for r in rows(data) if finite(r.get('metrics',{}).get(key)) and r.get('quality',{}).get(key,1)>0]
        result.append((key,len(values),statistics.mean(values) if values else None,min(values) if values else None,max(values) if values else None))
    return table(('sensor / 単位はキー末尾','有効sample','記述平均','最小','最大'),result)


def audit_value(audit,key):
    value=mapping(audit.get('gates')).get(key)
    return value is True


def generate(artifacts,output=None):
    evidence=Evidence(artifacts)
    lock=mapping(evidence.read('protocol_lock.json'));receipt=mapping(evidence.read('protocol_receipt.json'))
    collection=mapping(evidence.read('collection_manifest.json'));splits=mapping(evidence.read('split_manifest.json'))
    normalization=mapping(evidence.read('normalization_config.json'));probe_config=mapping(evidence.read('probe_config.json'))
    probe=mapping(evidence.read('probe_validation.json'));audit=mapping(evidence.read('independent_audit.json'))
    runtime=mapping(evidence.read('final_runtime_state.json'));resources=mapping(evidence.read('resource_summary.json'))
    mac=evidence.read('raw_mac_telemetry.jsonl');master=evidence.read('raw_master_telemetry.jsonl');frames=evidence.read('interoceptive_frames.jsonl')
    commits=evidence.read('commits.json');validation=evidence.read('validation_results.json')
    gate_known=mapping(probe.get('gate')).get('pass')
    probe_pass,probe_checks=probe_numerical_gate(probe)
    blocked=gate_known is False or probe.get('status')=='FAIL_AT_PROBE_GATE' or gate_known is True and not probe_pass
    downstream_names=('training_config.json','run_summary.json','checkpoint_manifest.json','test_results.json','final_checkpoint_results.json',
                      'counterfactual_results.json','live_results.json','live_order_effect.json','ood_results.json','mac_contribution.json')
    if blocked or not probe_pass:
        # Crucially do not open sealed D/test or downstream result files after a
        # failed/missing gate, even if a misleading file happens to exist.
        downstream={name:dict(status=NOT_RUN if blocked else 'NOT_RUN_GATE_UNRESOLVED',rows=[]) for name in downstream_names}
    else:downstream={name:mapping(evidence.read(name)) for name in downstream_names}
    train=downstream['training_config.json'];runs=downstream['run_summary.json'];test=downstream['test_results.json']
    live=downstream['live_results.json'];counter=downstream['counterfactual_results.json'];ood=downstream['ood_results.json']
    comparisons=mapping(test.get('comparisons'));live_comparisons=mapping(live.get('comparisons'));cf=mapping(counter.get('comparisons')).get('CF')
    modes=(('P1','TRAINED_BLIND'),('P2','SHUFFLED'),('P3','STALE'))
    fresh_records=bool(rows(mac)) and bool(rows(master)) and bool(rows(frames)) and all(r.get('source_kind')=='real' for r in rows(mac)+rows(master)+rows(frames))
    collection_sessions={r.get('session_id') for r in collection.get('sessions',[])}
    gates={
        'A_fresh_data':audit_value(audit,'fresh_data') and audit_value(audit,'split_disjoint') and fresh_records and {'A','B','C','D'}.issubset(collection_sessions),
        'B_probe':probe_pass and audit_value(audit,'probe_recomputed'),
        'C_P1_independent_BLIND':criterion(comparisons.get('P1')) and paired_rows(rows(test),'BODY','TRAINED_BLIND'),
        'D_P2_SHUFFLED':criterion(comparisons.get('P2')) and paired_rows(rows(test),'BODY','SHUFFLED'),
        'E_P3_STALE':criterion(comparisons.get('P3')) and paired_rows(rows(test),'BODY','STALE'),
        'F_counterfactual':criterion(cf) and sorted(r.get('seed') for r in rows(counter) if r.get('available',True))==list(range(12)) and any(finite(r.get('action_change_rate')) and r['action_change_rate']>0 for r in rows(counter)),
        'G_P4_live_independent_BLIND':criterion(live_comparisons.get('P4')) and paired_rows(rows(live),'BODY','TRAINED_BLIND'),
        'H_safety':resources.get('safety_pass') is True,
        'I_reproducibility':bool(lock.get('source_commit')) and bool(receipt.get('protocol_lock_commit')) and bool(receipt.get('protocol_lock_sha256')) and bool(normalization) and bool(downstream['checkpoint_manifest.json']) and all(audit_value(audit,key) for key in ('protocol_lock_valid','normalization_train_only','leakage_free','checkpoint_identity','initial_weights_equal','seed_pairing','primary_recomputed','live_recomputed','counterfactual_recomputed')),
        'runtime_cleanup':runtime.get('cleanup_pass') is True,
        'evidence_readable':not evidence.errors,
    }
    status='FAIL_AT_PROBE_GATE' if blocked else 'PASS' if all(gates.values()) else 'INCOMPLETE' if gate_known is None else 'FAIL'
    criteria=dict(schema_version='k0f2.success.v1',research_status=status,research_success='PASS' if status=='PASS' else 'FAIL',
                  gates=gates,unmet=[key for key,value in gates.items() if not value],n_seeds=12,minimum_effect=.01,
                  primary_rule='P1-P4 intersection-union: each n=12, mean>=0.01, CI lower>0, two-sided exact sign p<=.05',
                  downstream_status=NOT_RUN if blocked else 'NOT_RUN_GATE_UNRESOLVED' if not probe_pass else 'evaluated_if_present')
    best_rows=[r for r in rows(runs) if r.get('architecture')=='GRU128' and r.get('training_mode')=='BODY']
    best=dict(architecture='GRU128' if len(best_rows)==12 else None,n_seeds=len(best_rows),selection='validation-only, no architecture search',
              mean_validation_utility=mean(best_rows,'best_validation_utility'))
    summary=dict(schema_version='k0f2.report.v1',research_status=status,probe_checks=probe_checks,primary_comparisons=comparisons,
                 live_comparisons=live_comparisons,counterfactual_comparison=cf,best_core=best,
                 execution_complete=resources.get('execution_complete') is True and runtime.get('cleanup_pass') is True,
                 raw_statistics=dict(mac=raw_summary(mac),master=raw_summary(master)),source_hashes=evidence.inputs,
                 files_opened=evidence.opened,errors=evidence.errors,downstream_opened=probe_pass and not blocked)
    sections=[]
    def section(number,text):sections.append(f'## {number}. {SECTIONS[number-1]}\n\n{text}\n')
    stop_text='**予測validation gateが不合格のためFAIL_AT_PROBE_GATEを固定。Session Dのtest、新規policy学習、counterfactual、OOD、liveは未実施。数値を補完せず、これらの結果ファイルは開いていない。**' if blocked else '予測gate、固定12seed、P1–P4、反実仮想、安全・独立監査を全て満たしたときだけPASSとする。'
    section(1,f"**研究status: {status}。研究成功: {criteria['research_success']}。**\n\n"+stop_text+'\n\n'+comparison_table(comparisons,('P1','P2','P3'))+'\n\n'+comparison_table(live_comparisons,('P4',)))
    section(2,'K0-Fのコードfamilyを独立namespaceへ複製し、既存Fのraw、validation、test、normalization、checkpointを再利用しない。未来GPU使用率の予測から、task + 現在body + candidate actionを入力するaction-conditioned outcome probeへ変更した。主対照は独立学習BLIND。masked BODY-checkpoint BLINDは診断に留める。\n\n4 actionはRUN_CPU / RUN_RTX3060 / RUN_P100 / WAIT。WAITは20 ms待機後にRTX3060で同じjobを実行し、待機時間をlatencyへ含める。')
    section(3,table(('protocol identity','保存値'),(('実装source commit',lock.get('source_commit')),('lock commit',receipt.get('protocol_lock_commit')),('lock hash',receipt.get('protocol_lock_sha256')),('locked_at UTC',lock.get('locked_at_utc')),('独立監査のlock整合',mapping(audit.get('gates')).get('protocol_lock_valid'))))+'\n\nF2_PROTOCOL_LOCKを新規収集前にcommitして固定。12seed、ridge λ10、GRU128、240epoch、4action、utility式、最小効果0.01、shuffle/stale/OOD/liveをtest前に固定する。結果を見て変更・seed追加・architecture探索しない。protocol変更が必要ならF2をFAIL/invalidとして保全し、別phaseで扱う。')
    sessions=collection.get('sessions',[])
    section(4,table(('session','split','block数','decision数','開始','終了'),((r.get('session_id'),r.get('split'),r.get('blocks'),r.get('decisions'),r.get('start_timestamp'),r.get('end_timestamp')) for r in sessions))+'\n\nA/B=train、C=validation、D=test。各16block×32decision（4task×8履歴長）、全段階に進んだ場合の総数2048decision。A/B/Cからprobeを判定し、PASSかつpolicy凍結後だけDを新規取得する。FAIL時のD未収集を欠損失敗として埋めない。各sessionに安全な8種類のworkloadを含め、順序を固定seedでrandomize。row random splitは禁止。\n\n'+table(('split manifest','値'),mapping(splits.get('splits')).items()))
    section(5,table(('node','sample数','期間 秒','最大間隔 秒','実測source'),((name,info['samples'],info['span_seconds'],info['max_interval_seconds'],info['source_kinds']) for name,info in summary['raw_statistics'].items()))+'\n\nMac:\n\n'+sensor_table(mac,('thermal_pressure','cpu_utilization','memory_pressure','battery_fraction','network_rtt_ms','network_loss','daemon_cpu_fraction'))+'\n\n中央ノード:\n\n'+sensor_table(master,('cpu_temperature_c','cpu_utilization','memory_pressure','io_pressure','rtx3060_temperature_c','rtx3060_utilization','p100_temperature_c','p100_utilization','daemon_cpu_fraction'))+'\n\n以上は記述統計。sample数を統計上の独立nとして扱わない。Mac生温度は任意・未取得を許容し、熱圧を表示する。')
    section(6,f"保存frame数: {len(rows(frames))}。\n\n20個の値と20個のmask、timestamp、age、quality、source/receipt provenanceを保存する。task4と合計44floatをpolicyへ渡し、欠損の値0には必ずmask0を付ける。raw/aligned/frame/policy入力を別保存。frameの物理正規化定数とidentityは事前固定し、経験的fitを行わない。probeの特徴量スケーリングはtrainのみでfitする。\n\n"+table(('normalization config','値'),normalization.items()))
    section(7,'固定ridge λ10のaction-conditioned probe。taskとbody/mask、candidate actionからlog latencyとutilityを予測する。actual latency、future body、teacher/oracle、costを入力しない。candidate actionは入力、同actionの実測outcomeはtargetとして分離する。\n\n'+table(('probe config','値'),probe_config.items())+'\n\n'+table(('mode','target','split','MAE','正規化MAE','episode数'),((r.get('mode'),r.get('target'),r.get('split'),r.get('mae'),r.get('normalized_mae'),r.get('n_episodes')) for r in rows(probe))))
    section(8,'validationのlog latencyとutilityの両targetでBODY MAEがBLIND/SHUFFLEDより低く、予測utilityによる選択regretも両対照より低いことを要求。2種類以上の非定数targetを固定し、単一targetの改善だけでは通過しない。\n\n'+table(('固定gate','判定'),probe_checks.items())+'\n\n'+table(('mode','selection regret','選択正解率','episode数'),((r.get('mode'),r.get('selection_regret'),r.get('accuracy'),r.get('n_episodes')) for r in probe.get('selection_rows',[])))+'\n\n'+stop_text)
    section(9,('未実施: '+criteria['downstream_status'] if not probe_pass else table(('学習config','値'),((key,train.get(key)) for key in ('seeds','epochs','batch_size','learning_rate','selection','source_commit'))))+'\n\nGRU128 BODYと独立BLINDを同一初期重み・seed・task順・optimizer・240epochで学習する。12seed固定。lossはteacher cross entropy + expected utility regret。PPOは実施しない。validation utilityだけでbestを選択し、finalを別保存。GRU64探索は実施しない。\n\n'+table(('best Core','値'),best.items()))
    for number,key,control,explanation in ((10,'P1','TRAINED_BLIND','独立学習BLINDが主対照。同じBODY checkpointをmaskした比較はD_MASKED_BLINDとして診断に留める。'),(11,'P2','SHUFFLED','同task・同split/session・同sequence lengthで別blockへderangementし、body周辺分布とtask列を保持。self mappingを禁止しmapping保存。'),(12,'P3','STALE','事前固定30秒前のbodyを使用しageを通知。追加stale age curveをprimary判定へ混ぜない。')):
        section(number,explanation+'\n\n'+comparison_table(comparisons,(key,))+'\n\n'+metric_table(dict(rows=[r for r in rows(test) if display_mode(r) in ('BODY',control)]))+'\n\n'+('未実施: '+criteria['downstream_status'] if not probe_pass else '最小平均utility効果0.01、seed CI下限>0、exact sign p<=.05を全て要求。'))
    section(13,'task・weights・hidden・historyを同一にし、最終bodyだけA/Bへ分岐。全適格pairを評価し、改善pairの選別を禁止する。matched bodyのactionとfrozen actionを交換先の実測outcomeで比較する。行動変化に加え平均utility効果0.01以上、95% CI下限>0、exact sign p<=.05を要求する。\n\n'+comparison_table({'CF':cf} if cf else {},('CF',))+'\n\n'+table(('seed','pair数','action変化率','matched utility','frozen utility','効用差'),((r.get('seed'),r.get('n_pairs'),r.get('action_change_rate'),r.get('matched_utility'),r.get('frozen_utility'),r.get('utility_gain')) for r in rows(counter))))
    section(14,'新規session L1/L2各8block、12seed×4task×4条件で計3072jobを予定。凍結checkpointがBODY/TRAINED_BLIND/SHUFFLED/STALEの4条件で新規実ジョブを選択し、同base task/body group内の実行順をrandomize。body/decision/job start/end timestamp・mode順・温度・使用率を保存し、order effectを副次確認する。保存cost replayをlive成功の代用にしない。\n\n'+comparison_table(live_comparisons,('P4',))+'\n\n'+metric_table(live)+'\n\n'+table(('live order診断','値'),downstream['live_order_effect.json'].items()))
    section(15,'Mac追加価値はsecondaryとしてprimary成功条件から分離する。MASTER_ONLY対MAC_PLUS_MASTERが未実施なら分散身体感覚の追加価値を主張しない。\n\n'+metric_table(downstream['mac_contribution.json']))
    for number,category in ((16,'temporal'),(17,'sensor')):
        selected=[r for r in rows(ood) if r.get('category')==category]
        groups={mode:[r for r in selected if r.get('mode')==mode] for mode in {r.get('mode') for r in selected}}
        available={mode:mean(group,'utility') for mode,group in groups.items() if mean(group,'utility') is not None}
        worst=min(available,key=available.get) if available else None
        section(number,('sampling interval×2、更新遅延、jitter、dropout、stale30秒、task timingを固定条件で評価。' if category=='temporal' else 'noise、dropout、constant、scale、inversion、permutation、RTX/P100 sensor欠損、Mac欠損、network latency増加を評価。観測欠損と資源不在simulationは別条件。実GPUを危険に停止しない。')+'\n\n'+metric_table(dict(rows=selected))+f"\n\n最悪条件: {worst or '未実施'}、平均utility: {fmt(available.get(worst))}。OODはprimaryの積条件に加えないが、大幅崩壊は隠さない。")
    section(18,table(('独立監査gate','値'),mapping(audit.get('gates')).items())+'\n\n'+table(('到達段階と監査状態','値'),mapping(audit.get('stage_status')).items())+'\n\n到達済み範囲のaudit PASSと研究PASSを区別する。train/test overlap、timestamp/label/teacher/cost漏洩、train-only normalization、checkpoint選択、shuffle self mapping、初期重み一致、最終action再計算を独立経路で確認する。')
    section(19,'独立単位はtraining seed0–11。各seedの同じ評価task平均をpaired比較し、mean、sample SD、median、seed bootstrap95%CI、両側exact sign pを保存。telemetry sample/episode/jobを独立nとしない。P1–P4はintersection-union型で全成立を要求。各平均差>=0.01、CI下限>0、p<=.05。\n\nCIは共有した有限workload cohort上の学習seed変動を表し、未知装置・長期環境の母集団CIではない。probeは固定validation screeningであり、大量のaction outcomeを独立nとして有意差を作らない。')
    section(20,table(('安全確認','値'),(('safety_pass',resources.get('safety_pass')),('cleanup_pass',runtime.get('cleanup_pass')),('lockの安全設定',lock.get('safety'))))+'\n\nGPU/CPU温度stop、背景duty、GPU割当量、CPU worker上限をlockで固定。危険温度、OOM storm、thermal shutdown、電源断を誘発しない。詳細はresource_summary.jsonとfinal_runtime_state.jsonを参照。')
    section(21,'保存action cost行列は順次実測で、同時の物理反実仮想ではない。liveもmode順をrandomizeしているが時間経過による身体変化は残る。限られたtask、同じ装置、有限sessionでの結果を未知hardwareや一般的身体性へ拡張しない。Macの圧力proxyと生温度を混同しない。\n\n'+('probe失敗時には予測可能性・body情報・task設計・resource cost varianceの不足を検討するが、失敗原因を未実行のpolicy成績から推定しない。testを開いて原因を探さない。' if blocked else 'BODY情報の有無、対応、現在性、独立学習、live効果を別々に報告し、成立した一部だけで研究PASSにしない。'))
    section(22,table(('研究条件','値'),gates.items())+f"\n\n研究status: **{status}**。未実施は0や100%に置換しない。P1–P4のどれかが不成立なら全体PASSにはならない。")
    section(23,'全てのprimary、counterfactual、安全、fresh data、独立監査を通過した場合だけ、現在のMachine Interoceptionを利用した資源選択とheld-out/live task utility改善を限定的に主張できる。主観的な体温、意識、人間と同等の内受容、未知hardwareへの汎化は主張しない。\n\n'+('今回は全成功条件を確認した。' if status=='PASS' else '今回は研究成功を確認していない。理由をFAIL/未評価のまま保持する。'))
    section(24,'K0-G Visual Peripheral Senseへの進行推奨はF2 PASS時のみ。FAIL時はsensor情報量、task設計、outcome予測可能性、学習、時刻整合、resource cost分散へ原因を分解して報告する。camera、microphone、gazeを自動実装しない。次phaseの確認はcleanup後に一度だけ行う。')
    section(25,table(('再現identity','値'),(('source commit',lock.get('source_commit')),('lock commit',receipt.get('protocol_lock_commit')),('lock sha256',receipt.get('protocol_lock_sha256')),('normalization identity',normalization.get('identity',normalization.get('normalization_identity'))),('checkpoint manifest',downstream['checkpoint_manifest.json'].get('status','保存あり' if downstream['checkpoint_manifest.json'] else None))))+'\n\nraw/config/hash/seed/checkpoint/lockを保存し、接続情報はgitignore済みprivate設定に限定する。旧K0-F dataの再利用禁止。表示再生成:\n\n```sh\npython -m experiments.k0_f2_interoception_confirmatory.visualize --artifacts ARTIFACTS\npython -m experiments.k0_f2_interoception_confirmatory.report --artifacts ARTIFACTS\n```\n\n必須図: '+ '、'.join(f'[{name}]({name})' for name in PLOTS)+'。')
    section(26,table(('runtime','値'),runtime.items())+'\n\nsensor、training/probe worker、workload、GUI mirror、temporary SSH tunnel/socketを停止し、既存serviceを開始前へ戻す。GPU compute PID・温度・CPU load・Mac thermal stateを保存する。停止証拠なしをcleanup PASSとしない。')
    section(27,table(('commit','内容'),((r.get('hash'),r.get('subject')) for r in rows(commits)))+'\n\n外部pushなし。最終results commitを含む完全なcommit一覧は親agentが最終commit後に追記する。')
    destination=Path(output) if output else evidence.root;destination.mkdir(parents=True,exist_ok=True)
    (destination/'K0_F2_REPORT.md').write_text('# K0-F2 Machine Interoception Confirmatory Study\n\n'+'\n'.join(sections))
    for name,data in (('report_statistics.json',summary),('success_criteria.json',criteria)):(destination/name).write_text(json.dumps(data,ensure_ascii=False,indent=2,allow_nan=False)+'\n')
    return summary,criteria


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--artifacts',required=True,type=Path);p.add_argument('--output',type=Path);a=p.parse_args()
    summary,criteria=generate(a.artifacts,a.output)
    print(json.dumps({'research_status':criteria['research_status'],'unmet':criteria['unmet'],'downstream_opened':summary['downstream_opened']},ensure_ascii=False))
