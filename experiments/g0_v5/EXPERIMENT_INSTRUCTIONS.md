# Kamimusuhi G0-v5 — Predictive Invariant Grounding

**Target:** `llm_master`
**Experiment ID:** `G0-v5`
**Working directory:** `experiments/g0_v5/`
**Priority:** HIGH
**Primary purpose:** K0 / Continuity Kernel に渡せる、sensor / context に依存しすぎない latent-cause representation の獲得可能性を検証する。

---

## 0. 一文での目的

**観測系列と遷移だけから、同じ潜在原因を異なる context / dynamics / composition 下でも同一視できる内部表現を自己教師ありで獲得できるか検証する。**

G0-v5 では「分類精度」そのものを目的にしない。

見るべきものは、

> **表面的な sensor pattern ではなく、時間的・因果的に持続する latent cause が representation 上に残るか**

である。

---

# 1. 背景

K0-E / K0-E2 では、

* strict-memory
* returned information utilization
* GRU による状態保持

について一部成功した。

一方で、

* 未知時刻への転移
* OOD dynamics
* PPO 後の安定性
* context が変わっても同一内部状態として扱う能力

が弱い。

このため、次の段階で Core を複雑化する前に、

**「そもそも K0 が利用できる安定した latent representation を self-supervised に作れるか」**

を独立に検証する。

---

# 2. 仮説

## H1 — Temporal Predictability

将来観測を予測するよう学習した latent は、単純な reconstruction latent より latent cause を保持する。

## H2 — Context Invariance

同一 cause が異なる sensor context に現れた場合でも、predictive objective を持つモデルでは latent similarity が維持される。

## H3 — Intervention Selectivity

潜在原因だけを変更した intervention では latent が大きく変化し、無関係な sensor nuisance の変更には比較的小さい変化を示す。

## H4 — Compositional Generalization

学習中に存在しなかった cause × context の組合せに対しても latent identity が維持される。

---

# 3. 最重要禁止事項

## Training label leakage 禁止

以下の情報を **training loss / input / sampling weight / architecture selection に使用してはならない。**

* `cause`
* `context`
* canonical latent ID
* composition ID
* OOD category
* oracle correspondence
* evaluation-only intervention labels

これらは **evaluation script 内のみ** 利用可能。

特に、

```text
observation -> true cause
```

の supervised classification を混ぜないこと。

G0-v5 の意味が消える。

---

# 4. 比較対象

最低限以下を実装する。

## A. GRU Predictor Control

sequence encoder:

```text
obs_t
  ↓
GRU
  ↓
z_t
  ↓
predict obs_(t+k)
```

単純な predictive baseline。

---

## B. CPC / InfoNCE

positive:

```text
z_t ↔ future representation of same trajectory
```

negative:

```text
other trajectories / incompatible temporal positions
```

InfoNCE により predictive representation を形成。

---

## C. Temporal VICReg

時間的に対応する observation pair に対して、

* invariance
* variance
* covariance regularization

を適用。

representation collapse を明示的に防止する。

---

## D. JEPA-style Latent Predictor

pixel / observation reconstruction ではなく、

```text
context encoder
      ↓
predictor
      ↓
future latent
```

を予測する。

原則として target encoder は stop-gradient または EMA。

---

# 5. Mandatory Controls

各学習モデルに対して必ず比較する。

### C0 — Raw observation

latent learning なし。

### C1 — PCA

training observation のみで PCA fit。

### C2 — Untrained Twin

各 architecture と完全に同じ初期化モデルを **学習せず** 評価。

例:

```text
JEPA-trained
JEPA-untrained
```

を必ず対にする。

### C3 — old-G0

既存 G0 representation / encoder が利用可能なら固定 baseline とする。

### C4 — GRU Predictor

今回の neural predictive baseline。

---

# 6. Dataset

synthetic environment を使用する。

各 observation は概念的に、

```text
observation_t =
    f(
      latent_cause_t,
      sensor_context_t,
      nuisance_t,
      dynamics_t
    )
```

で生成する。

モデルには observation しか渡さない。

---

# 7. Dataset split

最低限以下の split を生成する。

```text
train
iid_test
midctx
match_ood
dynseg_ood
combo_oodctx
intervention
```

---

## 7.1 train

通常条件。

一部の、

```text
cause × context
```

組合せを意図的に欠落させる。

---

## 7.2 iid_test

train と同分布。

これは sanity check 用。

**主要成果として扱わない。**

---

## 7.3 midctx

episode の途中で sensor context を変更する。

例:

```text
t=0..23   context=A
t=24..47  context=B
```

latent cause は変更しない。

目的:

> context が変わった瞬間に latent identity が崩れないか。

---

## 7.4 match_ood

training では共起していない observation pair を与える。

異なる context に存在する同一 cause の representation が match 可能かを見る。

---

## 7.5 dynseg_ood

training と異なる、

* segment length
* transition timing
* persistence duration
* switching schedule

を使用する。

「何 step 後に変化する」という時間パターン暗記を検出する。

---

## 7.6 combo_oodctx

training で存在しなかった、

```text
cause × context
```

組合せ。

composition generalization の主要指標。

---

## 7.7 intervention

一つの生成因子だけを変更した counterfactual pair を作る。

### cause intervention

```text
cause A → cause B
context fixed
nuisance fixed
```

### context intervention

```text
context A → context B
cause fixed
```

### nuisance intervention

```text
nuisance A → nuisance B
cause fixed
context fixed
```

---

# 8. Main metrics

accuracy だけを使わない。

以下を保存する。

## 8.1 Cross-context matching AUC

異なる context にある latent pair が、

```text
same cause
different cause
```

を representation distance のみで判別可能か。

主要指標。

---

## 8.2 mid-context stability

context switch 前後で、

```text
cos(z_before, z_after)
```

または normalized distance を測る。

cause が同じなら representation が維持されること。

---

## 8.3 dynseg OOD AUC

未知 dynamics 下で matching を評価。

---

## 8.4 combo OOD AUC

未知 cause-context composition で matching を評価。

---

## 8.5 Intervention Selectivity

以下を比較する。

```text
Δz_cause
Δz_context
Δz_nuisance
```

理想:

```text
Δz_cause >> Δz_context
Δz_cause >> Δz_nuisance
```

定義例:

```text
IS =
distance(cause intervention)
/
(
 distance(context intervention)
 + distance(nuisance intervention)
 + eps
)
```

raw value も必ず保存する。

---

# 9. Collapse detection

以下を毎 epoch 記録。

```text
latent mean
latent std
per-dimension std
effective rank
covariance spectrum
pairwise cosine distribution
```

以下の場合は failure とする。

### Full collapse

ほぼ全入力が同一 latent。

### Dimensional collapse

大部分の dimension の variance が消失。

### Shortcut collapse

context ID だけで latent cluster が形成される。

---

# 10. Phase A — seed0 pilot

最初から5 seedsを回さない。

まず、

```text
seed = 0
```

のみ実行。

対象:

```text
GRU predictor
CPC
VICReg
JEPA
```

---

# 11. Pilot sanity gate

以下をすべて確認。

## Gate S1

training loss が finite。

```text
NaN = 0
Inf = 0
```

## Gate S2

latent full collapse が発生していない。

## Gate S3

trained model が **untrained twin と完全同等ではない**。

少なくとも IID または temporal prediction 指標で学習効果が確認できること。

## Gate S4

evaluation pipeline が oracle label を training 側へ漏らしていない。

## Gate S5

同一 checkpoint を2回評価した結果が tolerance 内で一致する。

---

# 12. Pilotでの停止条件

以下ならその method は本試験へ進めない。

```text
NaN / divergence
complete representation collapse
evaluation leakage
trained ≈ untrained on all metrics
obvious context-only shortcut
```

バグの場合は修正後 **seed0を最初から再実行**。

途中 checkpoint の継ぎ足しで済ませない。

---

# 13. Phase B — Full Run

pilot を通過した method のうち上位 **2〜3方式**を選択。

実行 seeds:

```text
0
1
2
3
4
```

architecture / hyperparameter は seed0 を見た後に変更しない。

変更する場合は、

```text
experiment revision
```

を上げること。

例:

```text
G0-v5.1
```

---

# 14. GPU allocation

GPU ごとに独立 process とする。

推奨:

```text
RTX 3060 12GB
  CPC
  JEPA

Tesla P100 16GB
  GRU
  VICReg
```

ただし速度実測後の再配置は可。

**結果は GPU の違いで混在させず hardware metadata を保存すること。**

---

# 15. Success criteria

## STRONG PASS

以下を満たす方式が1つ以上存在する。

1. `midctx`
2. `match_ood`
3. `dynseg_ood`
4. `combo_oodctx`

のうち **3/4以上**で、

```text
trained > untrained twin
trained > raw/PCA baseline
```

の改善方向が seed 間で一貫する。

加えて、

```text
intervention selectivity > baseline
```

であること。

### Seed consistency

主要結論について、

```text
>= 4 / 5 seeds
```

で同一方向を要求。

---

## WEAK PASS

OOD の一部では成立するが、

* dynseg
* composition
* intervention

のいずれかで崩れる。

この場合、

> representation mechanism 自体には信号あり

として次設計への入力にはするが、K0採用確定にはしない。

---

## FAIL

以下のいずれか。

* IID のみ改善
* context shortcut
* OOD で raw/PCA 以下
* untrained twinとの差が消える
* seed依存が極端
* latent collapse
* oracle leakage

---

# 16. 特に重要な解釈ルール

## IID成功 ≠ 成功

```text
IID AUC = 0.99
OOD AUC = 0.51
```

なら FAIL。

---

## Training loss低下 ≠ 成功

loss が綺麗に落ちても OOD invariant representation が得られなければ FAIL。

---

## Linear probe高精度 ≠ 成功

probe が sensor context を読んでいる可能性がある。

必ず、

```text
cross-context
OOD
intervention
```

を見る。

---

# 17. Reproducibility

各 run で以下を保存。

```yaml
experiment_id:
git_commit:
dataset_version:
model:
encoder:
predictor:
objective:
seed:
device:
gpu_name:
driver:
cuda:
pytorch:
hostname:
start_time:
end_time:
wall_clock:
config_hash:
```

config は実行前に hash 化。

実験中の config 書き換え禁止。

---

# 18. Directory structure

```text
experiments/g0_v5/
├── EXPERIMENT_INSTRUCTIONS.md
├── README.md
├── configs/
│   ├── gru.yaml
│   ├── cpc.yaml
│   ├── vicreg.yaml
│   └── jepa.yaml
├── src/
│   ├── dataset.py
│   ├── models/
│   ├── losses/
│   ├── train.py
│   └── evaluate.py
├── scripts/
│   ├── run_pilot.sh
│   ├── run_full.sh
│   └── summarize.sh
├── results/
│   ├── pilot/
│   ├── full/
│   └── controls/
├── plots/
├── manifests/
└── G0_V5_REPORT.md
```

---

# 19. Result format

1 run = 1 directory。

```text
results/full/
  jepa_seed0/
  jepa_seed1/
  ...
```

各 run:

```text
config.yaml
manifest.json
metrics.json
metrics.jsonl
train_log.jsonl
checkpoint_best.pt
checkpoint_final.pt
latent_stats.npz
eval_predictions.npz
```

---

# 20. metrics.json 必須項目

```json
{
  "iid": {},
  "midctx": {},
  "match_ood": {},
  "dynseg_ood": {},
  "combo_oodctx": {},
  "intervention": {},
  "collapse": {},
  "runtime": {},
  "failure_class": null
}
```

failure 時も directory を削除しない。

---

# 21. failure_class

最低限、

```text
NONE
NAN
OOM
COLLAPSE_FULL
COLLAPSE_DIM
SHORTCUT_CONTEXT
DATA_LEAK
EVAL_ERROR
TRAIN_DIVERGENCE
SYSTEM_ERROR
```

を使用。

---

# 22. 最終レポート

`G0_V5_REPORT.md` を生成。

必須構造:

```markdown
# G0-v5 Result

## Executive Summary

## Hypotheses
### H1
### H2
### H3
### H4

## Experimental Setup

## Controls

## Pilot

## Full 5-seed Results

## IID vs OOD

## Mid-context Transfer

## Cross-context Matching

## Dynamic-segment OOD

## Composition OOD

## Intervention Selectivity

## Collapse Analysis

## Failure Analysis

## Seed Stability

## Conclusion

## Decision for K0
```

---

# 23. グラフ

最低限生成。

### Plot 1

```text
method × OOD split
AUC
```

### Plot 2

```text
trained vs untrained twin
```

### Plot 3

```text
cause/context/nuisance intervention distance
```

### Plot 4

latent PCA / UMAP。

色分けを、

```text
cause
context
```

それぞれ別図で行う。

理想状態は、

> cause では構造が見え、context 色では混ざる

ことである。

---

# 24. Decision rule

最終的に以下のどれか一つを明示する。

```text
G0-V5_STRONG_PASS
G0-V5_WEAK_PASS
G0-V5_FAIL
G0-V5_INVALID
```

曖昧な「面白い結果だった」で終了しない。

---

# 25. Strong Pass 時の次段階

G0-v5 の encoder を freeze し、

```text
observation
    ↓
G0-v5 encoder
    ↓
persistent latent
    ↓
Continuity Kernel / K0-F
```

へ入力する。

次実験では、

* persistent state
* evidence accumulation
* attractor / working memory
* interruption tolerance
* unknown timing
* action policy

を評価する。

**G0-v5 内で policy learning は行わない。**

---

# 26. Weak Pass 時

失敗 split を明確化する。

例:

```text
midctx PASS
match PASS
dynseg FAIL
combo FAIL
```

なら、

> context invariance は獲得できるが compositional / dynamical generalization が不足

として次実験設計に渡す。

モデルサイズを増やすだけで解決しようとしない。

---

# 27. Fail 時

FAIL も成果である。

特に、

```text
CPC
VICReg
JEPA
GRU
```

がすべて、

```text
IID PASS
OOD FAIL
```

なら、

> observation predictionだけでは latent-cause grounding は生じない

という重要な結論になる。

その場合は G0-v5 で、

* explicit persistence pressure
* recurrence
* world-model factorization
* intervention-aware objective

等を検討する。

ただし **G0-v5 の途中で追加しない。**

---

# 28. 実行順

必ず以下の順序。

```text
[1] repository / branch確認
 ↓
[2] dataset generator固定
 ↓
[3] evaluator作成
 ↓
[4] controls実行
 ↓
[5] seed0 pilot
 ↓
[6] sanity gate
 ↓
[7] 上位2〜3方式をfreeze
 ↓
[8] seeds 0–4
 ↓
[9] 全OOD評価
 ↓
[10] aggregate
 ↓
[11] plots
 ↓
[12] G0_V5_REPORT.md
 ↓
[13] PASS / WEAK PASS / FAIL 判定
```

---

# 29. 実験開始前チェック

実行担当は開始前に以下を確認。

```text
[ ] training labels are hidden
[ ] evaluation oracle isolated
[ ] dataset seed fixed
[ ] git commit recorded
[ ] controls implemented
[ ] untrained twins implemented
[ ] configs immutable
[ ] results never overwritten
[ ] failure logs preserved
[ ] 3060 detected
[ ] P100 detected
```

---

# 30. llm_masterへの最終指示

この実験では「良い数字を出す」ことを目的としない。

最優先事項は、

1. leakage を防ぐ
2. control を置く
3. IID と OOD を分離する
4. untrained twin と比較する
5. seed 間再現性を見る
6. negative result を捨てない

ことである。

**G0-v5 の問いはただ一つ。**

> 観測の表層が変わっても維持される latent cause を、ラベルなし・予測学習のみから形成できるか？

これに `YES / PARTIAL / NO` のいずれかで答えられる実験にすること。


## Current execution scope (2026-09-13)

ユーザー追記: 独立したG0-v5として新設。旧G0-v4はread-onlyで歴史的結果を保存する。名称衝突により当初予定G0-v4からG0-v5へ変更。まずpreflight/scaffold、evaluator/controls、sanityを実装し、問題がなければseed0 pilotを開始する。Full runはpilot gate通過後の別段階として扱う。
