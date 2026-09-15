"""Pilot reporting only: never mistake one seed for five-seed evidence."""
from pathlib import Path
import json
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from .protocol import METHODS
ROOT=Path(__file__).resolve().parents[1]


def read(p): return json.loads(p.read_text()) if p.exists() else None


def main():
    rows={}; gates={}; controls={}
    for name in ('raw','pca'):
        controls[name]=read(ROOT/'results/controls'/name/'metrics.json')
    for m in METHODS:
        rows[m]=read(ROOT/'results/pilot'/f'{m}_seed0/metrics.json')
        controls[m+'_untrained']=read(ROOT/'results/controls'/f'{m}_seed0/evaluation/metrics.json')
        gates[m]=read(ROOT/'results/pilot'/f'{m}_seed0/sanity_gate.json')
    eligible=sorted([m for m in METHODS if gates[m] and gates[m].get('admitted')],
                    key=lambda m:gates[m]['label_free_relative_improvement'],reverse=True)
    selection={'experiment_id':'G0-v5','phase':'pilot','eligible':eligible,
               'proposed_top_methods':eligible[:3] if len(eligible)>=2 else [],
               'frozen_for_full':False,'full_run_started':False,
               'reason':'Current user scope ends at seed0 pilot; selection uses label-free validation, never OOD ranking'}
    (ROOT/'manifests/pilot_summary.json').write_text(json.dumps(selection,indent=2)+'\n')
    splits=('midctx','match_ood','dynseg_ood','combo_oodctx')
    def val(row,s):
        return row.get(s,{}).get('stability' if s=='midctx' else 'auc',float('nan')) if row else float('nan')
    text=['# G0-v5 Result','', '## Executive Summary','',
          'seed0 pilot段階。5-seed本判定は未実施であり、G0-V5_STRONG_PASS / G0-V5_WEAK_PASS / G0-V5_FAIL / G0-V5_INVALID の最終判定は保留。',
          '歴史的G0-v4の結果・checkpointは本実験に含めていない。', '',
          '## Hypotheses','### H1','予測目的とreconstructionの優劣は、今回reconstruction学習controlがないため直接検証していない。旧結果から補完しない。',
          '### H2','context invarianceはmidctxと距離matchingで測定。',
          '### H3','共通外生乱数でcause/context/nuisanceの一因子だけ変えた距離を比較。',
          '### H4','trainで欠落したcause×contextをcross-context queryに使用。','',
          '## Experimental Setup','',
          '1024 training episodes、独立validation、48 steps、24観測次元。dataset seed=20260913。共有cause成分とcontext依存成分を混合したsynthetic generator。方法比較は同一データ。完全任意なsensor変換に対する識別可能性は主張しない。',
          'Protocol/source/dataset hashes: `manifests/protocol_lock.json`。実行時hardware/source metadataは各runのmanifest。',
          '20 epochs、GRU/CPC/Temporal VICReg/EMA JEPA。bestはvalidation SSL lossのみで選択。学習入力はobsのみ。', '',
          '## Controls','',
          'raw / train-only PCA(16) / 各方式のexact initialization twin。旧G0は入力契約非互換と歴史的結果の流用禁止によりSKIP。', '',
          '## Pilot','', '|method|S1|S2|S3|S4|S5|admitted|stop|', '|---|---|---|---|---|---|---|---|']
    for m in METHODS:
        g=gates[m] or {}; checks=g.get('gates',{})
        text.append('|'+ '|'.join([m]+[str(checks.get(f'S{i}','N/A')) for i in range(1,6)]+[str(g.get('admitted','pending')),','.join(g.get('stop_reasons',[]))])+'|')
    text += ['', 'S3: train-only ridgeによる4-step future-observation readoutのvalidation MSEがexact twinより1%以上改善。EMA target driftのあるloss同士の比較だけでは通過させない。',
             '', '## Full 5-seed Results','未実行。pilot seed0は5-seedの代用ではない。', '', '## IID vs OOD','',
             '|representation|IID AUC|midctx cosine|match AUC|dynseg AUC|combo AUC|IS|','|---|---|---|---|---|---|---|']
    for name,row in {**controls,**rows}.items():
        if not row: continue
        values=[row.get('iid',{}).get('auc',float('nan'))]+[val(row,s) for s in splits]+[row.get('intervention',{}).get('selectivity',float('nan'))]
        text.append('|'+name+'|'+'|'.join(f'{v:.4f}' for v in values)+'|')
    for title,desc in [
        ('Mid-context Transfer','cause固定かつ既知compositionでcontextを切替。即時cosineと4-step recoveryを保存。'),
        ('Cross-context Matching','異contextの独立trajectoryペアを距離のみで判別。正負でcontextペア分布を一致させ、context-only controlのAUC=.5を確認。'),
        ('Dynamic-segment OOD','contextと許可cause集合を維持し、segment scheduleのみ変更。'),
        ('Composition OOD','学習で欠落したcause×contextをqueryに使用。'),
        ('Intervention Selectivity','cause/(context+nuisance+eps)、生距離も保存。比率だけでcollapseを成功扱いしない。'),
        ('Collapse Analysis','全epochのmean/std/per-dim std/covariance spectrum/entropy effective rank/cosine分布を保存。dense rank1も検出。'),
        ('Failure Analysis','全epochとtrain manifestの失敗をpilot gateへ反映。後の健康なbestで過去collapseを隠さない。失敗runも保存。'),
        ('Seed Stability','seed0のみ。4/5方向一致はまだ評価不能。'),
        ('Conclusion','pilotのsanityと科学的成功を分離。全OOD改善およびintervention優位のseed間再現性は未検証。'),
        ('Decision for K0','未採用。encoder freeze/K0統合は行っていない。')]:
        text += ['', '## '+title,'',desc]
    text+=['', '候補（label-free validation順）: '+(', '.join(eligible) or 'なし'),
           'Full対象のfreeze・seed1–4は未実行。2方式未満の場合は現行手順でFullへ進めない。',
           '', '## Provenance','[EXPERIMENT_LINEAGE.md](EXPERIMENT_LINEAGE.md)、[PREFLIGHT_AUDIT.md](../g0_v4/PREFLIGHT_AUDIT.md)参照。']
    (ROOT/'G0_V5_REPORT.md').write_text('\n'.join(text)+'\n')
    plots=ROOT/'plots';plots.mkdir(exist_ok=True)
    usable={n:r for n,r in {**controls,**rows}.items() if r and r.get('match_ood',{}).get('auc') is not None}
    if not usable:return
    names=list(usable); x=np.arange(len(names)); fig,ax=plt.subplots(figsize=(12,5))
    for i,s in enumerate(splits[1:]):ax.bar(x+(i-1)*.25,[val(usable[n],s) for n in names],.25,label=s)
    ax.axhline(.5,color='gray',ls='--');ax.set_ylabel('Distance AUC (seed0)');ax.set_xticks(x,names,rotation=45,ha='right');ax.legend();fig.tight_layout();fig.savefig(plots/'ood_auc.png');plt.close(fig)
    fig,axes=plt.subplots(1,4,figsize=(14,4))
    for ax,s in zip(axes,splits):
        ax.bar(np.arange(4)-.18,[val(rows[m],s) for m in METHODS],.36,label='trained')
        ax.bar(np.arange(4)+.18,[val(controls[m+'_untrained'],s) for m in METHODS],.36,label='exact twin')
        ax.set_xticks(np.arange(4),METHODS,rotation=40);ax.set_title(s);ax.legend()
    fig.tight_layout();fig.savefig(plots/'trained_vs_twin.png');plt.close(fig)
    fig,ax=plt.subplots(figsize=(12,5))
    for i,factor in enumerate(('cause','context','nuisance')):
        ax.bar(x+(i-1)*.25,[usable[n]['intervention'][factor+'_distance'] for n in names],.25,label=factor)
    ax.set_ylabel('Latent distance (scale varies by representation)');ax.set_xticks(x,names,rotation=45,ha='right');ax.legend();fig.tight_layout();fig.savefig(plots/'intervention_distances.png');plt.close(fig)
    for m in METHODS:
        p=ROOT/'results/pilot'/f'{m}_seed0/evaluation/eval_predictions.npz'
        if not p.exists():continue
        a=np.load(p); z=a['match_ood_pooled'];z=z-z.mean(0);_,_,vh=np.linalg.svd(z,full_matrices=False);xy=z@vh[:2].T
        for factor in ('cause','context'):
            fig,ax=plt.subplots(figsize=(5,4));sc=ax.scatter(xy[:,0],xy[:,1],c=a['match_ood_'+factor],s=8,cmap='tab10');fig.colorbar(sc,ax=ax);ax.set_title(f'{m} seed0 / {factor} (PCA display only)');fig.tight_layout();fig.savefig(plots/f'{m}_pca_{factor}.png');plt.close(fig)

if __name__=='__main__':main()
