# Kamimusuhi G0-v6 — CPC Grounding Replication

## 0. Mission

G0-v5で観測されたCPCの以下の現象が、seed依存の偶然ではなく再現可能なものか検証する。

G0-v5 seed0では、

* match_ood AUC: 0.6197 → 0.6906
* combo_ood AUC: 0.7131 → 0.7629
* Intervention Selectivity: 0.9208 → 1.0097
* midctx cosine: 0.9062 → 0.9796

と、untrained twinに対する改善が見られた。

一方で、

* dynseg AUC: 0.5421 → 0.4766
* IID AUC: 0.5846 → 0.5644
* future-observation linear readout: twin比20.98%悪化

となった。

G0-v6ではこの乖離を主対象とする。

---

# 1. Experiment ID

```text
G0-v6 — CPC Grounding Replication
```

新規ディレクトリ:

```text
experiments/g0_v6/
```

既存の、

```text
experiments/g0_v4/
experiments/g0_v5/
```

は変更禁止。

G0-v5のcheckpoint、result、FAIL判定を再利用・上書きしない。

---

# 2. Central Question

今回答える問いは以下。

> CPCはraw future observationの線形可読性を低下させながらも、contextを越えて持続するlatent cause structureを獲得しているのか？

さらに、

> その効果は複数seedで再現するのか？

を検証する。

---

# 3. Scope

今回は方式探索を行わない。

学習方式は原則、

```text
CPC trained
CPC exact untrained twin
```

のみ。

Controlとして、

```text
raw
PCA
```

を維持する。

新しいJEPA / VICReg / GRU方式を追加しない。

architecture searchも行わない。

---

# 4. G0-v5から引き継ぐもの

可能な限り以下はG0-v5と固定する。

* synthetic generator
* observation dimensionality
* episode length
* train/validation split思想
* OOD split定義
* matching evaluator
* intervention evaluator
* source/data/config hashing
* artifact preservation
* deterministic evaluation

ただし、G0-v5の**pilot admission ruleそのものは引き継がない**。

G0-v6は独立した仮説検証実験である。

---

# 5. Seeds

最初から本試験として以下を実行。

```text
seed = 0
seed = 1
seed = 2
seed = 3
seed = 4
```

seed0はpilot扱いしない。

5 seedsすべて同一config。

seed0を見てhyperparameterを変更しない。

変更が必要になった場合は、

```text
G0-v6.1
```

など別revisionとする。

---

# 6. Training Input

CPCには、

```text
observation sequence only
```

を入力する。

以下はtrainingに使用禁止。

* cause label
* context label
* nuisance label
* composition ID
* OOD label
* intervention type
* oracle correspondence

evaluation oracleとの境界を維持する。

---

# 7. CPC Objective

基本構造:

```text
obs_t
  ↓
encoder
  ↓
z_t
  ↓
autoregressive / temporal context
  ↓
c_t
  ↓
predict future latent
  ↓
InfoNCE
```

複数horizonを使う場合は事前固定する。

例:

```text
k = 1, 2, 4
```

G0-v5と同等条件を第一候補とし、理由なくobjectiveを変更しない。

---

# 8. G0-v6 Sanity Gate

G0-v5のS3は使用しない。

以下を新しいtraining health gateとする。

## S1 — Numerical health

```text
NaN = 0
Inf = 0
```

lossがfinite。

---

## S2 — Full collapse absence

全入力がほぼ同一representationになるfull collapseがない。

---

## S3 — Label-free temporal retrieval

主要sanity指標。

現在のrepresentation / contextから、

```text
正しいfuture latent
```

を、

```text
同batch / 他trajectory / hard temporal negatives
```

から識別できるか測る。

cause/contextラベルは使用しない。

trained CPCがexact untrained twinより改善していること。

候補指標:

```text
Recall@1
Recall@5
MRR
retrieval accuracy
```

少なくとも1つを事前にprimaryとして固定。

---

## S4 — No leakage

oracle情報がtrainingまたはadmissionに漏れていない。

---

## S5 — Reproducible evaluation

同一checkpointの再評価が許容誤差内で一致。

---

# 9. Important Change: Dimensional Collapse

G0-v5の、

```text
effective rank < 3.2
```

単独による即停止規則は使用しない。

effective rankは診断指標として保存するが、

```text
low effective rank == automatic failure
```

とはしない。

dimensional collapseの判定は少なくとも以下を組み合わせる。

* effective rank
* per-dimension variance
* covariance spectrum
* pairwise cosine distribution
* temporal retrieval
* trained vs twin
* perturbation sensitivity

低rankでもretrieval / selectivityが保たれている場合は学習を継続する。

---

# 10. Collapse Diagnostics

全epochで保存。

```text
latent mean
latent std
per-dim std
effective rank
covariance eigenvalues
pairwise cosine distribution
latent norm
InfoNCE loss
temporal retrieval metrics
```

latent dimension数との比率も保存。

---

# 11. Primary Evaluation

学習終了後、各seedについて以下を評価。

## E1 — Cross-context Matching

```text
match_ood AUC
```

主要指標。

---

## E2 — Composition OOD

```text
combo_ood AUC
```

主要指標。

---

## E3 — Dynamic Segment OOD

```text
dynseg_ood AUC
```

主要指標。

G0-v5で悪化したため特に重要。

---

## E4 — Mid-context Stability

context switch前後のlatent cosine / distance。

即時値とrecovery curveを保存。

---

## E5 — Intervention Selectivity

```text
cause distance
context distance
nuisance distance
IS
```

比率だけでなく生値を必ず報告。

---

## E6 — IID

IID AUCも保存するが、主判定にはしない。

---

# 12. Additional Key Test: Temporal Generalization

G0-v6では新たに、

```text
temporal retrieval IID
temporal retrieval OOD
```

を分離して測る。

少なくとも、

```text
train-like horizon
unseen horizon
altered segment timing
```

を比較。

目的は、

> CPCが時間間隔そのものを暗記しているだけなのか

を確認すること。

---

# 13. Future Observation Readout

G0-v5で使用したfuture-observation ridge readoutも**診断用として残す**。

ただし今回はsanity admissionには使用しない。

保存項目:

```text
future observation MSE trained
future observation MSE twin
delta %
```

これにより、

```text
temporal latent retrieval ↑
raw future readout ↓
```

が再現するか確認する。

これは今回の重要な研究対象。

---

# 14. Representation Geometry

各seedについて、

```text
PCA
```

を作成。

必要に応じてUMAPは補助的に使用可。

最低限、

```text
color = cause
color = context
color = time/segment
```

を別図にする。

ただしvisualizationだけで成功判定しない。

---

# 15. Controls

各seedで必ず、

```text
CPC trained
CPC exact untrained twin
```

を同じ初期stateから比較する。

確認:

```text
initial state_dict hash identical
```

raw/PCAはdataset seedごとに同じevaluation splitで評価。

---

# 16. Training Duration

G0-v5と同じ20 epochsを第一条件とする。

epoch途中でeffective rankが低下しても、full collapseや数値破綻がない限り即停止しない。

ただし、

```text
NaN
Inf
complete representation collapse
system failure
data leakage
```

が発生した場合は停止。

---

# 17. Statistical Decision

各主要指標について、

```text
delta = trained - untrained twin
```

をseedごとに計算。

最低限、

```text
mean
median
std
5 seed individual values
```

を出す。

可能ならbootstrap CIも計算。

---

# 18. Replication Criteria

## G0-V6_STRONG_PASS

以下を要求。

### A

match_oodで、

```text
trained > twin
```

が4/5 seeds以上。

### B

combo_oodで、

```text
trained > twin
```

が4/5 seeds以上。

### C

label-free temporal retrievalで、

```text
trained > twin
```

が5/5または少なくとも4/5。

### D

full collapseなし。

### E

ISまたはmidctxの少なくとも一方で改善方向が4/5 seeds以上。

---

## G0-V6_PARTIAL

cross-context / composition改善は再現するが、

```text
dynseg deteriorates
```

も一貫して再現する場合。

これは非常に重要な結果として扱う。

解釈候補:

> CPC learns context-stable identity-like structure but is brittle to temporal-dynamics shifts.

---

## G0-V6_FAIL

以下の場合。

* G0-v5 seed0のmatch/combo改善がseed間で再現しない
* trained ≈ twin
* temporal retrievalも改善しない
* representation collapse
* seed0のみの偶然と判断される

---

## G0-V6_INVALID

* leakage
* dataset bug
* evaluator bug
* config contamination
* reproducibility破綻

---

# 19. Do Not Do

以下は禁止。

```text
OOD結果を見てhyperparameter調整
cause labelでbest epoch選択
G0-v5 FAILの再判定
seed0だけで結論
effective rankだけで停止
良いseedだけ除外・採用
```

best checkpoint選択はlabel-free validation objectiveのみ。

---

# 20. Result Table

最終レポートにはseed別の表を必ず載せる。

例:

```text
seed | temporal_retrieval_delta | match_delta | combo_delta | dynseg_delta | IS_delta
0
1
2
3
4
mean
```

raw値も別表に保存。

---

# 21. Required Plots

最低限、

```text
trained vs twin — match_ood
trained vs twin — combo_ood
trained vs twin — dynseg
trained vs twin — temporal retrieval
future-readout MSE
effective rank trajectory
InfoNCE loss trajectory
intervention distances
```

を生成。

seedごとの点を消さず表示すること。

平均値だけにしない。

---

# 22. Directory

```text
experiments/g0_v6/
├── EXPERIMENT_INSTRUCTIONS.md
├── EXPERIMENT_LINEAGE.md
├── README.md
├── configs/
├── src/
├── scripts/
├── manifests/
├── results/
│   ├── controls/
│   └── full/
├── plots/
└── G0_V6_REPORT.md
```

---

# 23. Lineage

`EXPERIMENT_LINEAGE.md` に以下を記録。

```text
G0-v5
Broad predictive-invariant grounding screen
→ all methods failed predefined pilot admission
→ CPC nevertheless showed seed0 OOD improvement signals
→ no post-hoc admission rule modification

G0-v6
Independent replication experiment
→ CPC-only
→ predefined label-free temporal-retrieval health criterion
→ 5-seed replication
```

G0-v5のFAIL判定は変更しないことを明記。

---

# 24. Execution Order

```text
[1] G0-v6 scaffold
 ↓
[2] G0-v5 dataset/evaluator equivalence audit
 ↓
[3] temporal retrieval evaluator implementation
 ↓
[4] evaluator tests
 ↓
[5] protocol_lock.json
 ↓
[6] exact twins generation
 ↓
[7] seeds 0–4 CPC training
 ↓
[8] all evaluations
 ↓
[9] aggregate
 ↓
[10] plots
 ↓
[11] G0_V6_REPORT.md
 ↓
[12] STRONG_PASS / PARTIAL / FAIL / INVALID
```

---

# 25. llm_master Execution

利用可能な、

```text
RTX 3060 12GB
Tesla P100 16GB
```

へseedを並列配置してよい。

例:

```text
RTX3060:
seed0
seed2
seed4

P100:
seed1
seed3
```

ただしGPUによる差を追跡するため、

```text
gpu_name
device
CUDA
driver
wall_clock
```

をmanifestへ保存。

GPUが違ってもconfigは変更しない。

---

# 26. Final Research Question

最終レポートは以下に明示的に回答する。

> G0-v5 seed0で観測されたCPCのcontext-invariant grounding signalは、5 seedsで再現するか？

さらに、

> CPCによるlatent identity形成とraw future observation predictabilityは乖離するか？

> dynseg OOD悪化は再現可能な構造的弱点か？

の3点について、

```text
YES
PARTIAL
NO
```

のいずれかを出すこと。

結果が陰性でも、G0-v7へ無理に進めない。

**K0へのencoder統合はG0-v6の結果が出るまで禁止。**
