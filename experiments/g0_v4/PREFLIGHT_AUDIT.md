# G0-v4 preflight audit — 2026-09-13

## 状態

依頼文を EXPERIMENT_INSTRUCTIONS.md に原文保存。今回の学習・データ生成・GPU実験は未実行。既存結果は今回仕様の科学的判定に使用できない。既存実験の修正か独立した新規実験か、ユーザーへ確認中。

## 対象確認

- 現cwd: /Users/eightman/dev/sandbox/kamimusuhi
- branch: exp/u0-need-guided-memory
- HEAD: b198005ad82518fb5c676d2452b611b2714bc6b5
- remote: https://github.com/eightman999/kamimusuhi.git
- 作業開始時は未追跡 graphify-out/ のみ。保全済み。
- 発見した既存worktree: /Users/eightman/dev/sandbox/kamimusuhi-g0v4
- 既存branch: exp/g0-v4-predictive-invariant、HEAD c44ac29、git status clean。
- 既存レポート: experiments/g0_v4/reports/G0_V4_RESULTS.md。既存プロトコルで FAIL と記載、5-seed結果とseed0後のgridを含む。

## 今回仕様との差

| 項目 | 既存実装 | 今回要求 |
|---|---|---|
| 学習入力 | obs/next_obs/actions、action-conditioned encoder | observationのみ |
| matching | 原因別重心の対応accuracy | 異context個別ペア距離のsame/different cause AUC |
| midctx | 切替前ラベルで学習するprobeの切替後accuracy | cause固定・context切替前後latent安定性 |
| dynseg OOD | 同じ生成設定に対するsegment pooled特徴probe | 未知segment length/timing/persistence/schedule |
| composition | held-out cause×cause組合せと未見context | 意図的欠落cause×context組合せ |
| intervention | 重心方向をdecodeしoracle prototypeで評価 | 共通外生乱数で単一生成因子のみ変更したlatent距離 |
| health | aggregate variance/rank/cosine指標 | epochごとのmean/std/per-dim std/spectrum/分布 |
| provenance | best/latest・成功時meta、再構築twin | 保存済み初期twin、config hash、上書き禁止、失敗保存 |
| 判断規則 | +0.10やAUC .70等の別判定 | 4/5 seeds、3/4 OOD、raw/PCA/twin比較とselectivity |

根拠は既存worktreeの experiments/g0/evaluate.py:66,259,401,673、experiments/g0_v4/models.py、train.py、health.py、report.py、reports/G0_V4_RESULTS.md。sanitizeによるラベル除去は存在する。実際のoracle label leakageを検出したという主張ではない。

既存結果を変更・削除していない。既存プロトコルと今回仕様は同等ではないため、既存FAILから「今回の仮説はNO」と結論しない。修正再実行なら旧結果を保存し、protocol revisionとconfig/source/dataset hashを新たに固定する必要がある。

## 現在のremote状態

- llm_master と llm_master_now は同じSSHホスト設定。接続先hostnameは master。
- RTX 3060 12GB: 42 C、16 MiB使用、utilization 0%（照会時点）。
- GPU1 PCI 0000:06:00.0: nvidia-smi Unknown Error。P100の正常認識は確認できず。
- 既存Python runtime: /home/eightman/dev/k0-venv-complete/bin/python、torch 2.13.0+cu126、CUDA device_count 1、NVML warning。
- llama-master/open-webui等の既存サービスを確認。停止・再起動・GPUリセットは実行していない。
- 初回SSHでは既存ProxyCommandがWake-on-LANを送信しbanner timeout。以後はProxyCommand=noneで直接接続。

## 検証

既存worktreeで次を実行（既存機能のテストであり今回仕様準拠の証明ではない）:

```sh
/Users/eightman/dev/sandbox/kamimusuhi/.venv/bin/python -m pytest experiments/g0_v4/tests -q -p no:cacheprovider
```

結果: 10 passed in 7.28s。既存成果物の再学習・再評価・GPU実験は未実施。

同名worktree発見前に作成した未実行draft 7ファイルは .local/g0-v4-preflight-drafts-20260913/ に隔離。学習開始前の草稿であり、固定済み実験実装ではない。
