# Kamimusuhi — 2026-09-17 ワールドモデル/VLAイベントまとめ

## 位置づけ

2026-09-17 のイベントで得た、Kamimusuhi
（永続AI / 非言語Core / self model / persistent memory / persona）に関係する知見をまとめる。

---

## 1. イベントで得た重要な観測

### 1.1 「心情」を World Model の対象にできるか

イベント中、物理世界の次状態だけでなく、

- 人間の心情
- 表情
- 声色
- personality / persona
- 行動傾向

のような外見に直接現れない状態をWorld Modelで扱えるか、という議論があった。

その場では Emotional World Model と呼ばれる研究方向が紹介され、
ドラマ動画などから表情・声色を使って感情状態を扱う例が話題になった。

**Transcript:** 44:20–48:00, 54:05–54:35

### 1.2 Persona-conditioned behavior

特定人物のプレイ履歴を大量に学習し、その人物らしいプレイだけを引き出す、
というpersona conditioningの話が出た。

ここでは単なる「文体模倣」ではなく、

- 同じ状況に対する選択
- 行動方針
- preference
- behavioral pattern

を条件付けする発想になっている。

**Transcript:** 45:14–46:08

### 1.3 Prediction error = surprise

ロボットやWorld Modelが予測した状態と実際の状態が食い違うとき、
そのイベントは「surprise」として扱えるという議論があった。

さらに、

> 予測が外れた場面だけ取り込んで新しく学習する

という継続学習の方向が話題になった。

**Transcript:** 46:08–47:07

これはKamimusuhiのmemory updateに非常に近い。

### 1.4 latent representation を中心に置く考え方

イベントでは、世界をピクセルのまま再生成するより、
latent representation 上で予測した方が効率がよい可能性が議論された。

Kamimusuhiに置き換えると、
「文章として全部自己説明してから考える」のではなく、

> 非言語Core内部の状態遷移を先に持ち、必要なときだけ言語化する

という設計とよく対応する。

**Transcript:** 03:16–04:29, 38:11–40:12

---

## 2. Kamimusuhiへの設計示唆

### 2.1 Surprise-driven Memory Promotion

現在のmemory設計に以下を追加する価値がある。

```text
experience
↓
K0 prediction
↓
actual observation
↓
prediction error / surprise
↓
importance score
↓
episodic memory promotion
```

すべてを保存するのではなく、
「自分の予測を破ったもの」を優先して残す。

候補スコア:

```text
importance =
  prediction_error
  × novelty
  × self_relevance
  × social_relevance
```

---

### 2.2 K0を「次に何をするか」だけでなく「次に何が起きるか」も予測させる

K0の現在の action-selection 系に world-model head を追加する案。

入力:

- internal state
- recent observation
- memory retrieval
- previous action
- language-return signal

出力:

- action logits
- predicted next latent state
- uncertainty
- optional expected reward/homeostatic delta

これによりK0に、

- surprise
- anticipation
- counterfactual
- active learning trigger

を持たせられる。

---

### 2.3 Self model を予測モデルとして再定義できる

self model を単なる属性DBにせず、

> 「自分はこの状況でどう行動するはずか」

を予測するモデルとして扱う。

例:

```text
state + relationship + memory
→ expected self action / preference
```

実際の選択との差を、

- self drift
- preference change
- identity update candidate

としてC0に渡す。

---

### 2.4 Personaは文章ではなく policy 側に焼く

イベントのpersona-conditioned behaviorの発想は、
Kamimusuhiの「キャラ焼きLLM」と接続できる。

重要なのは、

> persona = 語尾や口調

ではなく、

> persona = 状態に対する一貫した選択傾向

とすること。

評価対象:

- same-context choice consistency
- preference stability
- relationship-conditioned behavior
- risk tolerance
- curiosity
- language-call frequency
- memory-use tendency

---

## 3. K0/C0への具体的タスク案

### K0-G candidate: Predictive Core

追加head:

1. action head
2. next-state predictor
3. uncertainty head

評価:

- next latent prediction
- shuffled observationとの差
- delayed observation
- surprise detection
- unseen transition generalization

### C0 extension: Surprise Gate

C0 proposalを生成する条件に、

- high prediction error
- repeated surprise
- stable behavioral deviation

を入れる。

単発の例外はself modelを書き換えず、
繰り返し観測された場合だけupdate candidateにする。

---

## 4. Emotional World Modelの扱い

今日のイベントでは関連研究の存在が紹介されたが、
この transcript だけでは具体的な論文・実装・性能までは確認できていない。

したがって現時点では、

**イベント由来の探索キーワード**

として扱う。

調査語:

- emotional world model
- affective world model
- emotion prediction from multimodal video
- social world model
- theory of mind world model
- latent human state prediction
- personality-conditioned agent policy

---

## 5. 直近の実装候補

### Minimal Surprise Buffer

既存K0 runtimeに、

```python
prediction
actual
error
timestamp
context_id
memory_refs
```

だけ保存する。

最初は学習しなくてよい。

まず、

- 何がsurpriseになるか
- surpriseが後の行動に効くか
- memory retrievalと相関するか

をログで観察する。

---

## 6. 現時点の結論

Kamimusuhiへの最大の持ち帰りは、

> **記憶すべきなのは、起きたこと全部ではなく「自分の世界モデルを破った出来事」ではないか。**

そして、

> **persona / self は文章上の設定ではなく、未来の行動を予測するモデルとして扱える。**

この2つ。

K0を「反射 + action selector」から
**predictive non-verbal core** に進化させる候補として有望。
