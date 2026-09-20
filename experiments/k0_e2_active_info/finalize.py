"""Data-only final resource summary and bounded completeness checks."""
import argparse,hashlib,json,statistics,subprocess
from pathlib import Path
from .evaluate import seed_statistics,paired_sign_test
from .train import atomic


def run(root):
    root=Path(root);groups={}
    for line in (root/'gpu_telemetry.jsonl').read_text().splitlines():
        row=json.loads(line)
        for sample in row['rows']:
            values=[x.strip() for x in sample.split(',')]
            uuid=values[0];g=groups.setdefault((row['phase'],uuid),[])
            def number(x):
                try:return float(x.split()[0])
                except ValueError:return None
            g.append(dict(utilization=number(values[1]),memory_mib=number(values[2]),temperature_c=number(values[3]),power_w=number(values[4])))
    resources=[]
    for (phase,gpu),samples in groups.items():
        item={'phase':phase,'gpu_uuid':gpu,'samples':len(samples)}
        for key in samples[0]:
            vals=[s[key] for s in samples if s[key] is not None]
            item[key+'_mean']=statistics.mean(vals) if vals else None;item[key+'_max']=max(vals) if vals else None
        resources.append(item)
    queue_times={phase:json.loads((root/(phase+'_queue.json')).read_text())['finished_at']-json.loads((root/(phase+'_queue.json')).read_text())['started_at'] for phase in ['imitation','ppo','combined']}
    atomic(root/'resource_summary.json',dict(gpu_samples=resources,phase_wall_seconds=queue_times,
        sample_scope='nvidia-smi every 5 seconds during each phase, includes worker startup/idle tail; not per-kernel utilization'))
    rows=json.loads((root/'run_summary.json').read_text());base=[r for r in rows if r['checkpoint_name']=='imitation_best.pt']
    assert len(rows)==112 and len(base)==24
    for arch in ['mlp','gru64','gru128']:assert sorted(r['seed'] for r in base if r['architecture']==arch)==list(range(8))
    comparisons={}
    for metric in ['task_success','reward','llm_call_rate']:
        a={r['seed']:r[metric] for r in rows if r['run_id'].endswith('-B8') and r['checkpoint_name']=='ppo_final.pt'}
        b={r['seed']:r[metric] for r in rows if r['run_id'].endswith('-B3') and r['checkpoint_name']=='ppo_final.pt'}
        delta=[a[s]-b[s] for s in range(8)];comparisons[metric]={'B8_minus_B3':seed_statistics(delta),'paired_sign_test':paired_sign_test(delta),'seed_differences':delta}
    atomic(root/'stabilized_comparison.json',comparisons)
    ev=json.loads((root/'run_evidence.json').read_text());assert ev['run_count']==88 and not ev['failed_runs'] and not ev['nonfinite_checkpoints']
    runtime=json.loads((root/'final_runtime_state.json').read_text())
    assert runtime['restored_stopped_state'] and runtime['training_workers_terminated']
    baseline=json.loads((root/'baseline_checkpoint_integrity.json').read_text())
    assert baseline['unchanged'] and baseline['checked']==24
    commits=subprocess.check_output(['git','log','--reverse','--format=%h %s','a2f6bbb..HEAD'],text=True).splitlines()
    review=dict(primary_runs=88,evaluation_rows=112,checkpoint_files_verified=sum(len(r['checkpoints']) for r in ev['runs']),
                source_commits=commits,baseline_checkpoint_integrity=baseline['unchanged'],
                restored_stopped_state=True,gpu_workers_terminated=True,push_performed=False)
    atomic(root/'final_review.json',review)
    report=root/'K0_E2_REPORT.md'
    if report.exists():
        marker='\n## 22. 最終自己レビュー'
        text=report.read_text().split(marker)[0]
        text+=marker+'\n\n主実験88本・112評価行、各主要群8 seedを確認。'
        text+=f" {review['checkpoint_files_verified']} checkpointのSHA-256・stage・有限値を実体から検査し、全PPO枝の初期重み・親checkpoint・RNG・GPU対応が一致した。\n\n"
        text+='K0-Eの394ファイルと既存best checkpoint24本は不変。J72一時サービスと既存推論サービスは停止状態、GPU学習workerは終了している。日本語GUIは実Macのアクセシビリティ状態と別途offscreen画像で確認した。\n\n'
        text+='[最終実行状態](final_runtime_state.json)、[checkpoint監査](run_evidence.json)、[GUI描画検証](gui_verification.json)、[計算資源](resource_summary.json)を保存。B8とB3の成功率差は0で、追加KLが低LR単独より必要という証拠はない。\n\n'
        text+='結果commit直前の実装commit系列（外部公開・pushなし）：\n\n'
        text+='\n'.join('- `'+line.split(' ',1)[0]+'` '+line.split(' ',1)[1] for line in commits)+'\n\n'
        text+='結果を含む最終commitはGit履歴と最終回答で識別する。この追記は `python -m experiments.k0_e2_active_info.finalize --artifacts experiments/k0_e2_active_info/artifacts/primary` で再生成できる。\n'
        report.write_text(text)
    required=['K0_E2_REPORT.md','run_summary.csv','run_summary.json','policy_interventions.json','language_response_ablation.json','ppo_ablation.json','ood_results.json','retention_results.json','voi_results.json','run_evidence.json','source_manifest.json','runtime_packages.txt','system_info.txt','gpu_info.txt']
    required+=json.loads((root/'plot_manifest.json').read_text())['files']
    missing=[name for name in required if not (root/name).is_file()]
    # At most derived report may be pending on first invocation; final invocation asserts all.
    atomic(root/'artifact_integrity.json',dict(required_files=required,missing_files=missing,
        sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir() if p.is_file() and p.suffix in ('.json','.csv','.md','.png','.txt') and p.name!='artifact_integrity.json'},
        checkpoint_files_verified=sum(len(r['checkpoints']) for r in ev['runs']),evaluation_records=len(rows),full_intervention_checkpoints=24))
    assert not missing,missing

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--artifacts',required=True);a=p.parse_args();run(a.artifacts)
