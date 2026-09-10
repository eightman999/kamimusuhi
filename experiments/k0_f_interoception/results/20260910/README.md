# K0-F 保存結果

研究成功 **FAIL**。予測probe不合格後の方策学習・評価は探索的診断。結果と制約は [K0_F_REPORT.md](K0_F_REPORT.md) を参照。

`k0-f-artifacts.tar.gz` は実測raw、整列frame、policy入力、48block dataset、v1/v2 probe、16runのbest/final計32checkpoint、ablation、counterfactual、OOD、新規1280実ジョブ、34実LLM生成記録、失敗した初回収集、停止確認、図を含む。機械可読hashは `artifact_manifest.json`。実験namespace内へ展開すると `artifacts/` が復元される。既存成果物を上書きせず新しい空ディレクトリで検証する。

```sh
# リポジトリの同じsourceを使い、新しい空ディレクトリに復元
mkdir -p restored-k0-f
tar -xzf k0-f-artifacts.tar.gz -C restored-k0-f
python -m experiments.k0_f_interoception.visualize --artifacts restored-k0-f/artifacts/review
python -m experiments.k0_f_interoception.report --artifacts restored-k0-f/artifacts/primary_v2 --policy-artifacts restored-k0-f/artifacts/analysis_v2 --live-artifacts restored-k0-f/artifacts/live_diagnostic --output restored-k0-f/report
```

55 unit tests PASS、実sensor／実job／native日本語GUIを別々に確認。全owned daemon・worker・GUI・sleep抑制を停止し、GPU compute空・通常温度・既存service状態一致を確認。Git pushは実施していない。
