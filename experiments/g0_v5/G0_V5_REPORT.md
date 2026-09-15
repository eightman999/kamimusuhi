# G0-v5 Result

## Executive Summary

**判定: G0-V5_FAIL — seed0 pilotの事前定義sanity gateで全方式が不通過。**

5-seed試験は停止規則に従い未実施。この判定は固定した今回の構成・pilot admissionに限定し、予測学習一般でgroundingが不可能という結論にはしない。
歴史的G0-v4の結果・checkpointは本実験に含めていない。

## Hypotheses
### H1
予測目的とreconstructionの優劣は、今回reconstruction学習controlがないため直接検証していない。旧結果から補完しない。
### H2
context invarianceはmidctxと距離matchingで測定。
### H3
共通外生乱数でcause/context/nuisanceの一因子だけ変えた距離を比較。
### H4
trainで欠落したcause×contextをcross-context queryに使用。

## Experimental Setup

1024 training episodes、独立validation、48 steps、24観測次元。dataset seed=20260913。共有cause成分とcontext依存成分を混合したsynthetic generator。方法比較は同一データ。完全任意なsensor変換に対する識別可能性は主張しない。
Protocol/source/dataset hashes: `manifests/protocol_lock.json`。実行時hardware/source metadataは各runのmanifest。
20 epochs、GRU/CPC/Temporal VICReg/EMA JEPA。bestはvalidation SSL lossのみで選択。学習入力はobsのみ。

## Controls

raw / train-only PCA(16) / 各方式のexact initialization twin。旧G0は入力契約非互換と歴史的結果の流用禁止によりSKIP。

## Pilot

|method|S1|S2|S3|S4|S5|admitted|stop|
|---|---|---|---|---|---|---|---|
|gru|True|True|False|True|True|False|COLLAPSE_DIM|
|cpc|True|True|False|True|True|False||
|vicreg|True|True|False|True|True|False|COLLAPSE_DIM|
|jepa|True|True|False|True|True|False|COLLAPSE_DIM|

S3: train-only ridgeによる4-step future-observation readoutのvalidation MSEがexact twinより1%以上改善。EMA target driftのあるloss同士の比較だけでは通過させない。

## Full 5-seed Results
未実行。pilot seed0は5-seedの代用ではない。

## IID vs OOD

|representation|IID AUC|midctx cosine|match AUC|dynseg AUC|combo AUC|IS|
|---|---|---|---|---|---|---|
|raw|0.5750|0.3306|0.5846|0.5712|0.7320|0.8790|
|pca|0.5757|0.1626|0.5855|0.5732|0.7324|0.8809|
|gru_untrained|0.5846|0.9062|0.6197|0.5421|0.7131|0.9208|
|cpc_untrained|0.5846|0.9062|0.6197|0.5421|0.7131|0.9208|
|vicreg_untrained|0.5846|0.9062|0.6197|0.5421|0.7131|0.9208|
|jepa_untrained|0.5846|0.9062|0.6197|0.5421|0.7131|0.9208|
|gru|0.5268|0.9134|0.5406|0.4972|0.5842|0.6929|
|cpc|0.5644|0.9796|0.6906|0.4766|0.7629|1.0097|
|vicreg|0.5451|0.9333|0.5832|0.4999|0.5882|0.7443|
|jepa|0.5461|0.9489|0.5864|0.4942|0.5952|0.7393|

## Mid-context Transfer

cause固定かつ既知compositionでcontextを切替。即時cosineと4-step recoveryを保存。

## Cross-context Matching

異contextの独立trajectoryペアを距離のみで判別。正負でcontextペア分布を一致させ、context-only controlのAUC=.5を確認。

## Dynamic-segment OOD

contextと許可cause集合を維持し、segment scheduleのみ変更。

## Composition OOD

学習で欠落したcause×contextをqueryに使用。

## Intervention Selectivity

cause/(context+nuisance+eps)、生距離も保存。比率だけでcollapseを成功扱いしない。

## Collapse Analysis

全epochのmean/std/per-dim std/covariance spectrum/entropy effective rank/cosine分布を保存。dense rank1も検出。

## Failure Analysis

全epochとtrain manifestの失敗をpilot gateへ反映。後の健康なbestで過去collapseを隠さない。失敗runも保存。

## Seed Stability

seed0のみ。4/5方向一致はまだ評価不能。

## Conclusion

G0-V5_FAIL（pilot gate）。GRU/VICReg/JEPAはepoch2でentropy effective rankが32次元に対する閾値3.2を下回り停止。CPCは20epoch完走しnative validation InfoNCEが4.6499→0.2346に低下したが、事前定義S3の共通future-observation readout MSEはuntrained twinより20.98%悪化した。

CPCにはmatch_ood AUC 0.6906（twin 0.6197）、combo AUC 0.7629（twin 0.7131）、IS 1.0097（twin 0.9208）のseed0改善信号がある。一方dynseg AUCは0.4766（twin 0.5421）、IID AUCは0.5644（twin 0.5846）。「全指標で学習効果なし」とは解釈しない。OOD改善を見てS3を事後変更せず、Fullへの昇格を停止した。

full collapseはなく、次元collapseの判定は保守的な事前閾値に依存する。epoch2停止から長時間学習後の回復可能性までは否定できない。5 seedsでの方向一致、reconstruction controlを含むH1、K0への実際の転移は未検証。

## Decision for K0

未採用。encoder freeze/K0統合は行っていない。

候補（label-free validation順）: なし
Full対象のfreeze・seed1–4は未実行。2方式未満の場合は現行手順でFullへ進めない。

## Provenance
[EXPERIMENT_LINEAGE.md](EXPERIMENT_LINEAGE.md)、[PREFLIGHT_AUDIT.md](../g0_v4/PREFLIGHT_AUDIT.md)参照。

## Verification and artifacts

- local focused tests: 20 passed。Python compileall、shell syntax検査PASS。
- raw/PCA/4 untrained twinsの再評価一致、pilot4方式のS1/S2/S4/S5 PASS。全方式のS3 FAIL。
- controlsと実学習の初期state_dict完全一致。学習終了後source/data/config hash再照合PASS。
- remote/local結果106ファイルのSHA-256一致を確認。`manifests/artifact_hashes.json`参照。
- RTX3060/P100それぞれCUDA演算・実pilotを実行。実行後44°C/65°C、利用率0%、現bootにXidなし。これは長時間負荷安定性の証明ではない。
- historical G0-v4 worktreeはgit status cleanのまま。旧結果・checkpoint・FAIL判定を変更していない。
- remote: `/home/eightman/dev/g0-v5-20260913/experiments/g0_v5/`。base commit `b198005ad82518fb5c676d2452b611b2714bc6b5` + `manifests/protocol_lock.json`で未コミット新規実装を識別。
- plots: `plots/ood_auc.png`, `plots/trained_vs_twin.png`, `plots/intervention_distances.png`, 各方式 `plots/*_pca_cause.png` / `plots/*_pca_context.png`。

本節と最終gate解釈は固定学習コードの実行後に観測済み出力から追記した。実行コード・config・datasetは変更していない。`summarize.sh`はpilot集計を再生成するため、最終注記を含む本レポートは別途保存してから実行すること。
