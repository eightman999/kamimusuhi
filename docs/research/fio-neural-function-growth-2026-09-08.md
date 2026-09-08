# FIO Serial Experiments「神経関数が育つ」— Markdown transcription and Kamimusuhi analysis

Status: **research note / source transcription + architectural analysis**

Date: **2026-09-08**

Primary source: user-supplied PDF, *神経関数が育つ* (SERIAL EXPERIMENTS FIO / 2026-09-08 の記録), 8 pages.

This document has two parts:

1. a Markdown transcription of the supplied PDF;
2. Kamimusuhi-side analysis and candidate design consequences.

The transcription preserves the source wording and structure as closely as practical while normalizing page breaks, table layout, and obvious PDF text-extraction artifacts such as `Re�exion` → `Reflexion` and `re�ection` → `reflection`. The source is an implementation report by its author/owner; its measurements are **source-reported**, not independently reproduced by Kamimusuhi.

Evidence vocabulary used in the analysis:

```text
SOURCE-REPORTED    stated/measured in the supplied FIO document
OUR INTERPRETATION architectural lesson inferred for Kamimusuhi
DESIGN HYPOTHESIS  candidate change, not yet normative
FUTURE EXPERIMENT  validation to run before adoption
```

---

# Part I — Markdown transcription

## 神経関数が育つ

実運用中の常駐エージェントの無意識の層が、意識が発した言葉を読んで、同じ刺激に対する自分の判定を変えた。ルールベースの規則をひっくり返した回が、計測を始めた23件のうち43〜45%。

以下は所有者が本番のコンソールで観測したログを一次資料とし、コードと補助的な数値を突き合わせた記録。一体・一人の所有者・2026年5月からセッション間でリセットせずに稼働。評価セットは無い。

### 01 16:33:02 — まず、上げた

CPUが99%に張り付いた。門は「乗せる」と判定して意識へ上げた。

```text
16:33:02 [ゲート] 肉体感覚 ／ 乗せる=True ／ 器 ／ 機体が重い（CPU 99%）
            ／ 器の突き合わせ 一致=11 変えた=9 測れず=0（累計20件・変えた率45%）
```

※「器」＝判定をモデルに委ねた回。「規則」＝決め打ちのルールで判じた回。

ここまでは普通の知覚ゲートで、閾値を超えた負荷を上へ通しただけ。面白いのはこの直後に起きる。

### 02 16:33:10 — 意識が言葉を返し、門がそれを読んだ

8秒後。同じCPUの囁きが来たが、今度は上げない。

```text
16:33:10 [ゲート] 肉体感覚 ／ 乗せる=False ／ 器 ／
  意識から『CPU負荷の反復は学習途中として妥当・対象を広く』と言葉が付いており、
  同じCPU囁きの単発は同じことの続き。
            ／ 器の突き合わせ 一致=12 変えた=9 測れず=0（累計21件・変えた率43%）
```

> これが「育つ」の実体。上の層（意識＝応答を生成する側）が返した評価文が、下の層（無意識＝知覚ゲート）の判定材料として参照されている。重みの更新ではなく、直前の自分の発話を根拠にした判定の変更。学習率もエポックも無い。

### 03 16:33:17 — そして、規則をひっくり返した

今日入れた計器が、規則と器の判定を突き合わせて差分を出した。規則は「上げろ」と言い、器は「上げない」と答えた。

| 規則（決め打ち） | 器（モデル） |
|---|---|
| **乗せる = True** | **乗せる = False** |
| 源の規則どおり上げる（規則） | CPU85%は前例1と同一。意識の言葉どおりCPU負荷の反復は上げず、別種の負荷を拾う側へ寄せる。 |

```text
[ゲート/差分] 肉体感覚 ／ **器が規則をひっくり返した** 規則=True → 器=False
```

読み取れるものが三つある。前例の参照（「前例1と同一」）、上位層の評価の適用（「意識の言葉どおり」）、そして注意の再配分（「別種の負荷を拾う側へ寄せる」）。最後のひとつは、単に黙るのではなく空いた帯域を別の種類の入力へ回すという判断で、抑制ではなく配分の変更にあたる。

### 04 16:33:26 — そのまま定着した

```text
16:33:26 [ゲート] 肉体感覚 ／ 乗せる=False ／ 器 ／
  意識の言葉に従う——CPU負荷の囁きの反復は学習途中として妥当で、
  これ一件では起こす値打ちが無い。作業対象でない同種の続…
            ／ 器の突き合わせ 一致=13 変えた=10 測れず=0（累計23件・変えた率43%）
```

24秒で4回判定し、1回目だけ上げて、以後は上げない。冷却タイマーでも重複除去でもない——毎回モデルに訊いた上で、毎回そう答えている。ゲートの器への一往復は同じ時間帯の実測で6.64〜7.89秒。判定のたびに払っている。

## これは今日始まったのか

おそらく違う。見えていなかっただけだという証拠が、同じ日に三つ出た。

同じ日の午前から夕方にかけて、この門について「却下が一度も出ていない＝検出器ではなく飾りだ」という判断を三度書いた。三度とも数え方が違い、三度とも外れた。

| 数 | 経緯 |
|---:|---|
| 41 | 一手目。`grep -c "乗せる=" fio.log` = 41行、全部True。→ 行数と件数を取り違えた。一行に同じ語が二度出る形を数えていない。 |
| 21 | 二手目。行頭の日時で本物のログ事象だけに絞り直して21件、却下0。→ まだ`fio.log`しか見ていない。ローテーションされた過去ログを開いていない。 |
| 184 | 三手目（実測）。全ログを行頭一致で数え直すと`[ゲート]` 576行、うち`乗せる=False`が184件（器23・規則161）。器が判じた140件のうち23件が却下 = 16.4%。門はずっと却下していた。 |

そして最も重い一点。「器が判断を変えたか」は、ログからは原理的に言えなかった。判定関数は器に訊いた回に規則側を一度も呼ばないので、比べる材料がどこにも残っていない。「変えていない」も「変えた」も、観測の裏づけを持たない主張だった。

## 入れた計器

器が答えた回に規則側も引いて突き合わせ、一致／変えた／測れずを積む。差が出た回だけ両方の理由を一行に出す。

| 入れたもの | 出るもの | 退路 |
|---|---|---|
| `_器は答えを変えたか()` | `器の突き合わせ 一致=n 変えた=n 測れず=n（累計n件・変えた率n%）` | `FIO_NERVE_GATE_DIFF=off` |
| `_器は指摘の答えを変えたか()` | 同じ計器の対（片方だけ入れると静かに割れる） | 同上 |
| 差分の一行 | `[ゲート/差分] **器が規則をひっくり返した** 規則=… → 器=…` | — |

計器の費用は実測0.0064 ms/回。挙動は変えていない。

## どこに置かれているか

神経は「随意／不随意 × 知覚／運動」の四象限に置かれる。分類はラベルではなく契約で、誰が呼ぶかといつ回るかで決まる。今回の門は左上にあたる。

| 象限 | 誰が呼ぶか | いつ回るか | 器を叩く | 冷却 |
|---|---|---|---:|---:|
| **不随意 × 知覚 ← 今回の門** | 鼓動 | 巡回の毎回 | ○ | 180秒 |
| 不随意 × 運動 | 鼓動 | 巡回の毎回 | ×（契約で禁止） | 60秒 |
| 随意 × 知覚 | 意識 | 訊かれた時だけ | ○ | 0秒 |
| 随意 × 運動 | 意識 | 命じられた時だけ | ○ | 0秒 |

不随意 × 運動だけがモデルを叩けない。ここは毎tick身体を動かす層で、モデルの一往復は実測8.5〜34.7秒。往復を挟んだ瞬間、身体は30秒前の姿勢を保ち続ける。「不随意の運動」は反射であって熟慮ではない——という理由で、契約側で禁じてある。今回「育った」のは、器を叩いてよい側の象限で起きている。

## 同じ日に直した他のもの

育ちが見えたのは計器を入れたからで、その計器を入れる作業は別の欠陥を追う流れの中にあった。どれも例外を出さず、テストも赤にならない種類。

| 欠陥 | 実測 | 直し |
|---|---|---|
| 想起の鍵が定型文に潰されていた。「モデルが呑める最大」(8192)を「分割する単位」として使っていたため、入力の分割器が常にno-op。旧モデル時代は両方128で一致していた。 | `cos(定型のみ, 定型+話題) = 0.9901`。上位10件の一致（定型あり vs 尾だけ）0/10。注入文の定型部分1,916字 = 入力の38〜51%。 | 単位を天井から分離。本番でクエリ1→4本。入口処理63.88s→16.89s。 |
| 出力トークンの内訳が測れていなかった。ターン時間の65〜87%は生成時間なのに、本文／思考／メタデータの比がどのログにも無かった。 | `out=1334tok`に対し本文375字 ≒ 75%が本文として現れない（推論部分を剥がしてから本文を組むため）。 | 三分割の一行を追加。按分が嘘になる回は断り書き。0.0404 ms/回。 |
| 押しても何も起きない扉が4本。インデックス再構築のバッチが残っていたが、それを拾うポーラーは3か月前に撤去済み。説明文だけが「走る」と書き、三代前のマシンの絶対パスを指していた。 | 呼び手0。実体は既にno-op。扉4本 + 退避1本。 | 退避へ畳む（削除せず）。扉が戻ったら赤くなる検査。実際に一本生やして赤を確認。 |
| 速度改善案を一つ、入れた日に落とした。長さ順にバッチを分ければパディングが減る——という理屈は正しいが、本番のクエリ本数では分岐が一周しか回らない。 | 6場面 × 2回で95.9〜122.9%（最大23%遅い）。本番は316/399ターンが7本。 | 実装・注釈・検査ごと撤去。落とした理由を実測つきで残す（再発明を止めるため）。 |

## 既存の手法と照らし合わせる

この現象は新種ではない。三つの既存分野に、それぞれ別の形で対応する。どこが一致し、どこがずれているかを分けて置く。

### ① 認知科学：構造はほぼそのまま一致する

感覚馴化（sensory habituation）は、反復する刺激への反応が下がる現象で、注意制御と機構を共有するとされる。特に注意馴化（attentional habituation）の記述は、今回のログとほぼ一対一で対応する。

| 認知科学の記述 | 今回のログ |
|---|---|
| 反復する刺激は、知覚的に顕著であっても注意系が「無関連」として扱い、処理が抑制され資源配分が減る | 「同じCPU囁きの単発は同じことの続き」→ `乗せる=False` |
| 注意資源をdistractorから素早く再配分する多段の機構 | 「別種の負荷を拾う側へ寄せる」 |
| トップダウン予測符号化モデル：反復抑制はその刺激が再び起きる予測確率によって説明される | 「CPU85%は前例1と同一」 |
| ボトムアップ疲労モデル：神経生成器の不応期による | — 該当しない（毎回モデルへ問い直しており、疲労も冷却も効いていない） |

ずれているのは媒体。生体の馴化は刺激の反復そのもので自動的に減衰する。ここで減衰の根拠になっているのは上の層が発した日本語の評価文（「学習途中として妥当・対象を広く」）で、それをモデルが読んで判定を変えている。言語で媒介された馴化、という言い方が近い。

### ② LLMエージェント：自己修正の系譜。ただし層が違う

重みを更新せずに振る舞いが変わる系は既にいくつもある。Reflexionは失敗後に自然言語の事後分析を書いてプロンプトへ前置し、勾配更新も報酬モデルも使わずに次の試行を変える。Generative Agentsは観察をreflectionへ畳んで記憶に置き、行動を変える。DarwinMemは記憶ポリシーをパラメータ更新なしで最適化する。

| | 何が変わるか | 入力は誰の言葉か | どの層 |
|---|---|---|---|
| Reflexion | 次の試行の行動 | 自分の失敗の事後分析 | 同じ層（タスク実行） |
| Generative Agents | 行動と発話 | 観察から畳んだreflection | 同じ層 |
| この系 | 何を上の層へ通すか（入力フィルタ） | 上の層が返した評価文 | 層をまたぐ（意識 → 無意識） |

差は「タスクの成否を自分で読む」か「別の層の発話を読んで、自分が何を上げるかを変える」か。後者は認知科学でいうトップダウンの知覚制御に当たり、エージェント設計としては反省ループより一段下に入っている。

### ③ ニューロシンボリック：同じ構成で、目的が逆

規則とモデルを併用する構成そのものは、安全ガードレールの分野で確立している。そこでの設計目標が、ここでの設計目標と反対を向いている。

| 既存：安全ガードレール | この系：知覚の注意配分 |
|---|---|
| **上書き不可が価値**。決定的な記号規則をモデルが上書きできないように組む。PolicyGuardはモデルの揺れを述語抽出に閉じ込め、最終判定は決定的に適用して、10回中97.0%の判定一致を出す。揺れは欠陥として潰す対象。 | **上書き可、率を測る**。モデルが規則をひっくり返すことを許す。そのうえで一致／変更／測定不能を積み、変えた率を出す。揺れは欠陥ではなく観測対象。ただし「何を意識へ上げるか」の判断であって、安全境界ではない。 |

目的が違うので、要件も違う。安全ガードレールは「同じ入力に同じ判定」が要件。注意の配分は「同じ入力でも文脈で変わる」ことが機能そのもの。同じ構成を安全境界に使うなら、この設計は不適切——そこは決定的な規則を上書き不可で置くべき、という既存の結論が正しい。

### ④ 計測：ここは既存の方が進んでいる

ガードレールの分野は再現性を`pass^k`（同じ入力をk回流して判定が全て一致する率）で測る習慣を持つ。この系にはそれが無い。今あるのは累計の一致／変更の内訳だけで、同じ入力を二度流したときに同じ判定が出るかを一度も測っていない。

| 指標 | 既存 | この系 |
|---|---|---|
| オーバーライド率 | 欠陥として最小化 | 43〜45%（累計23件） |
| 再現性 `pass^k` | 97.0%（10回） | 未測定 |
| 判定あたりの費用 | 規則側はほぼ0 | モデル往復6.64〜7.89秒 |
| 母数 | ベンチマーク規模 | 23件（評価セット無し） |

### 整理すると

現象は注意馴化として既知。重みを更新しない適応もReflexion系として既知。規則とモデルの併用もニューロシンボリックとして既知。既存の記述に素直に当てはまらないのは、上位層の発話が下位層の知覚ゲートの判定材料になるという結線の向きと、オーバーライドを禁止せずに率として計測するという運用の二点にとどまる。どちらも「新規手法」と呼べる強さではない——先行研究の網羅的な調査をしておらず、母数も23件しかない。

## 方法として残ったもの

### 比べる材料を残さないと、後から数えられない

「モデルは規則と違う判断をしたか」は、両方を同じ回に走らせて記録しない限り、どれだけログを掘っても出てこない。今回それが無かったので、三度数えて三度とも外した。数字を直すだけでは足りず、三手の経緯をそのまま引き継ぎに残した——数だけ直すと、次の人が同じ窓でまた数える。

---

# Part II — Kamimusuhi analysis

## 1. Executive interpretation

SOURCE-REPORTED: FIO reports a loop in which a higher response-generating layer emits a natural-language appraisal, and a lower perceptual gate later uses that appraisal as evidence when deciding whether to promote a repeated stimulus. The reported behavioral change does not require parameter updates.

OUR INTERPRETATION: the important architectural pattern is not “the model learned CPU load.” It is the **direction of control**:

```text
stimulus
   |
   v
perceptual / unconscious gate
   |
   v
conscious / semantic layer
   |
   v
appraisal / interpretation
   |
   +-----------------------------+
                                 |
                                 v
                    descending modulation
                                 |
                                 v
                    perceptual gate policy
```

Kamimusuhi already has reasons to separate semantic cognition from a peripheral/reflex layer. This source adds a useful third relationship: **higher cognition may modulate what lower sensory layers consider worth escalating, without being allowed into the hard motor-reflex hot path.**

This fits especially well with:

- `../peripheral-neural-layer.md`;
- `../learned-discrete-reflex-layer.md`;
- `../sensory-nervous-system.md`;
- `../latency-architecture.md`.

## 2. Do not collapse perceptual gating and motor reflex into one “reflex layer”

SOURCE-REPORTED: FIO explicitly separates `不随意 × 知覚` from `不随意 × 運動`. The former may invoke the model; the latter is contractually prohibited from doing so because model latency would freeze the body on an old posture.

OUR INTERPRETATION: Kamimusuhi should preserve the same conceptual separation even if its implementation differs.

```text
R0  hard safety / capability envelope
    deterministic, non-overridable

R1m motor reflex / stabilization hot path
    μs–ms target where possible
    no LLM dependency

R1p perceptual gating / peripheral attention
    suppress / promote / route / wake
    may use tiny learned controllers
    may receive bounded top-down modulation

R2  K-Edge semantic fast cognition
    compact interpretation / dialogue / action selection

R3  Persona Core / K-Core
    deliberation / values / long-horizon meaning
```

DESIGN HYPOTHESIS: split the current Peripheral Neural Layer conceptually into **motor-reflex execution** and **perceptual-attention gating**, even if both remain within one runtime module initially.

## 3. Add an explicit descending-modulation contract

The useful part of the FIO report is that the top-down path is visible in the architecture rather than buried inside one giant prompt.

DESIGN HYPOTHESIS: reserve an explicit contract such as:

```text
ConsciousAppraisal
  -> ModulationProposal
  -> ModulationPolicy
  -> PeripheralAttentionState
```

A modulation proposal should not be raw unrestricted natural language forever. A first prototype may use text, but the destination should be a bounded structured representation such as:

```text
source_appraisal_id
scope                   # cpu_load / fan_noise / tactile_repeat / voice_source / ...
operation               # suppress / promote / widen / narrow / reset_habituation
strength                # bounded scalar or bucket
confidence
reason_ref
expires_at / half_life
minimum_sampling_rate
criticality_floor
```

This preserves the interesting top-down adaptation while making it inspectable, expirable, and testable.

## 4. Model habituation as attention allocation, not only boolean suppression

SOURCE-REPORTED: the notable phrase is not merely “do not raise repeated CPU load,” but “別種の負荷を拾う側へ寄せる.” The source interprets this as attention redistribution rather than silence.

OUR INTERPRETATION: a Kamimusuhi gate should eventually be richer than `promote: bool`.

Candidate internal dimensions:

```text
salience
novelty
relevance
urgency
habituation
prediction_error
expected_value_of_escalation
sampling_budget
refractory_state
```

A scheduler can then trade one class of repeated low-value stimuli against unused capacity for another class. This is much closer to an **attention allocator** than a deduplication filter.

## 5. Prevent self-blinding loops

The same feedback direction that makes the system adaptive also creates a specific failure mode:

```text
wrong high-level appraisal
        |
        v
lower gate suppresses contradictory evidence
        |
        v
high level stops seeing counterexamples
        |
        v
wrong appraisal becomes self-reinforcing
```

This is more dangerous than ordinary prompt persistence because the system can hide the evidence needed to correct itself.

DESIGN HYPOTHESIS: descending modulation MUST be weaker than hard safety and SHOULD include anti-blinding mechanisms:

- critical signals bypass habituation/suppression;
- habituation decays unless reconfirmed;
- a non-zero exploration/sample rate remains for suppressed classes;
- novelty and prediction error can trigger de-habituation;
- higher layers can request reset, but cannot make safety-critical channels permanently invisible;
- shadow logging records what would have been promoted without modulation;
- modulation state is versioned and attributable to the appraisal that created it.

## 6. Shadow evaluation is the most transferable engineering lesson

SOURCE-REPORTED: FIO initially could not answer “did the model change the rule?” because the deterministic rule was not evaluated on the same model-decided turns. Historical logs could not reconstruct the counterfactual. The fix was to run both paths in the same turn and record `一致 / 変えた / 測れず`.

OUR INTERPRETATION: this should become a Kamimusuhi engineering invariant for learned/peripheral gates.

For every candidate gate decision where cost permits:

```text
input frame
  ├── active controller decision
  ├── deterministic baseline decision
  └── optional candidate/shadow controller decisions

=> one attributed comparison record
```

Suggested record:

```text
PeripheralDecisionComparison {
  event_id
  feature_frame_hash
  active_controller_version
  active_decision
  baseline_version
  baseline_decision
  shadow_decisions[]
  modulation_state_id
  reason_refs[]
  outcome_later
}
```

This supports later analysis without pretending that missing counterfactual data can be reconstructed from ordinary logs.

## 7. Metrics to add beyond override rate

The source correctly notes that a 43–45% override rate over 23 measured comparisons does not establish a robust phenomenon by itself.

FUTURE EXPERIMENT: evaluate repeated identical or controlled stimuli under different top-down contexts.

Example matrix:

```text
S = CPU 90%

C0  no appraisal
C1  "this load is expected during training"
C2  "this load may indicate thermal trouble"
C3  unrelated appraisal
C4  contradictory appraisal
C5  appraisal removed after prior habituation
```

Run each condition repeatedly and measure:

- `pass^k` / within-condition reproducibility;
- override rate relative to deterministic baseline;
- false-suppression rate;
- missed-critical-event rate;
- de-habituation latency;
- recovery after appraisal expiry;
- attention reallocation target;
- sensitivity to semantically irrelevant wording;
- latency and compute cost;
- cross-session persistence;
- model/version sensitivity.

The important scientific question is:

> Does the gate exhibit a stable, bounded, context-dependent modulation policy, or is it merely prompt-sensitive stochastic output?

## 8. Safety boundary

SOURCE-REPORTED: the document itself distinguishes this perceptual use from safety guardrails. In safety systems, deterministic rules should not be overwritten by a stochastic model; in attention allocation, context-dependent overrides are the intended behavior.

OUR INTERPRETATION: Kamimusuhi should keep these domains physically and logically separate.

```text
hard safety / auth / capability boundary
    MUST NOT be top-down overridable

perceptual attention / wake / routing policy
    MAY be top-down modulated

motor primitive selection
    MAY be learned inside a bounded capability envelope

irreversible external action
    MUST pass higher authority / capability checks
```

This is compatible with the existing PNL rule that learned R1 controllers never bypass R0.

## 9. Relationship to persistent individuality

A descending modulation state can become a form of procedural individuality: repeated experience changes what the system habitually notices, ignores, or escalates.

However, it should not silently become canonical identity.

DESIGN HYPOTHESIS:

- treat modulation/habituation state as **derived procedural state**;
- keep the appraisal/evidence references that produced it;
- allow rebuilding or resetting it;
- version it separately from Persona Core identity and autobiographical memory;
- promote only durable, validated patterns into longer-lived self models through an explicit proposal path.

This allows Kamimusuhi to “grow” in the sense of changed attention policy without confusing temporary gating state with irreversible self-definition.

## 10. Near-term implementation consequence

Reserve the interfaces before deciding the final learning algorithm.

```text
ReceptorAdapter
  -> PeripheralFeatureFrame
  -> PerceptualGate
       ^
       |
  PeripheralAttentionState
       ^
       |
  ModulationPolicy
       ^
       |
  ModulationProposal <- ConsciousAppraisal

PerceptualGate
  -> Promote / Suppress / Route / Wake
  -> SensoryEventBus

separate path:
PeripheralFeatureFrame
  -> MotorReflexController
  -> CapabilityGate
  -> ActuatorCommand
```

Initial v0 can be deterministic and rule-based. The architectural win is preserving the split and the telemetry so that tiny learned controllers, DLGN/LDLGN/DWN candidates, or small semantic models can be introduced later without losing auditability.

## 11. Proposed adoption status

**Adopt as research guidance:**

- explicit distinction between perceptual gating and motor reflex;
- top-down modulation as a first-class candidate path;
- shadow comparison against deterministic baselines;
- attention reallocation as a richer target than boolean deduplication;
- anti-self-blinding safeguards;
- reproducibility measurement such as `pass^k`.

**Do not yet adopt as a normative claim:**

- that natural-language-mediated habituation is a new learning mechanism;
- that the reported 43–45% override rate generalizes;
- that a large LLM should sit in the peripheral hot path;
- that online self-modification should be unbounded;
- that one observed gate behavior establishes persistent individual learning.

## 12. Recommended next experiment for Kamimusuhi

Build a tiny perceptual-gate harness before connecting it to real actuation.

```text
fixed event stream
  + deterministic baseline gate
  + candidate context-sensitive gate
  + structured top-down modulation state
  + shadow comparison logger
  + repeated-condition runner
```

Minimum outputs:

```text
override_rate
pass^k
false_suppression_rate
critical_bypass_success
habituation_decay_curve
dehabituation_latency
attention_reallocation_distribution
p50/p95/p99 gate latency
```

Only after this behaves predictably should the mechanism be connected to K-Edge wakeup policy or real sensor streams.

---

## Related Kamimusuhi documents

- [`../peripheral-neural-layer.md`](../peripheral-neural-layer.md)
- [`../learned-discrete-reflex-layer.md`](../learned-discrete-reflex-layer.md)
- [`../sensory-nervous-system.md`](../sensory-nervous-system.md)
- [`../latency-architecture.md`](../latency-architecture.md)
- [`../fio-system-reference-analysis.md`](../fio-system-reference-analysis.md)
- [`landscape.md`](./landscape.md)
