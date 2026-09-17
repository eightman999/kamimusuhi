# MIOBA / FlyBrain — 2026-09-17 イベントまとめ

## 位置づけ

ワールドモデル / Physical AI / 継続学習の議論から、
MIOBA / FlyBrain に持ち帰れる設計上の示唆を整理する。

---

## 1. イベント由来の重要な示唆

### 1.1 「ハエ脳 + RL」だけでは意味が薄い

イベント後半の会話で、ハエ脳をゲーム等に使う話に対して、

> それはただの強化学習では？
> ハエでやる必要があるのか？

という指摘があった。

さらに、

> ドローンなど実際の身体につないで飛ばす方が、まだ「ハエらしい」

という意見も出た。

**Transcript:** 2:30:12–2:31:12

これは重要な批判。

FlyBrainの価値を出すには、

- biological structureを使う必然性
- embodied sensorimotor loop
- temporal dynamics
- fast reflex
- low-power / local processing
- adaptive behavior

のどれかを示す必要がある。

---

## 2. MIOBAへの示唆

### 2.1 World Modelより「fast predictive reflex」

MIOBA / FlyBrain は巨大なWorld Modelを目指す必要はない。

むしろ、

```text
sensory stream
↓
very small recurrent state
↓
short-horizon prediction
↓
reflex / orient / avoid
```

の方が差別化しやすい。

予測対象:

- next sensory vector
- collision risk
- motion onset
- novelty
- expected reward/homeostasis delta

---

### 2.2 Surpriseを神経活動トリガーにする

イベントで出た prediction error = surprise をFlyBrainへ適用。

```text
predicted sensory state
vs
actual sensory state
```

の差を、

- attention
- neuromodulation
- plasticity gate
- memory writing
- language escalation

に使える。

Kamimusuhi K0 と同じ考え方を、生物模倣系ではより低層で使う。

---

### 2.3 Body0を強くする方向は正しい

MIOBAが「脳だけ」のベンチマークへ進むと、
通常のRNN/RLとの差が出にくい。

Body0のように、

- body dynamics
- environment
- sensor feedback
- action feedback
- closed loop

を必須にした方が意味がある。

イベントのPhysical AI議論でも、
最終的な価値は実世界に接続したときの安全性・適応性にあるという話が強かった。

---

## 3. 次にやるべき実験候補

### Experiment A — Prediction-error reflex

FlyBrainに1-step sensory predictorを追加。

比較:

- predictorなし
- predictorあり
- surprise-gated attentionあり

指標:

- hazard response latency
- false positive
- energy / update count
- new-situation adaptation

### Experiment B — Drone-like 2D agent

Beat Saber系ではなく、

- optic flow
- obstacle avoidance
- target orientation
- gust disturbance
- light / temperature preference

のような「ハエらしい」環境を優先。

### Experiment C — Neuromodulated plasticity

prediction error が一定値を超えた時だけplasticityを上げる。

目的:

- 常時学習を避ける
- surprise eventだけ高速適応
- catastrophic driftを抑える

---

## 4. 研究主張の候補

弱い主張:

> ハエ脳構造を使ってゲームを解けた

より、

> 生物由来の小規模回路を、予測誤差駆動の高速sensorimotor controllerとして使うと、
> 未知外乱への反応を少ない更新で改善できる

の方が研究として意味を作りやすい。

---

## 5. Kamimusuhiとの接続

MIOBA:
- fast loop
- sub-symbolic
- short-horizon prediction
- sensorimotor surprise

Kamimusuhi K0:
- slower core loop
- memory aware
- action arbitration
- language-call decision

J系:
- explanation
- planning
- abstraction

という階層化が自然。

---

## 6. 現時点の結論

イベントから最も重要な指摘は、

> **「ハエの回路を使った」だけでは研究上の意味は弱い。**

したがって、
MIOBA / FlyBrain は **embodiment + temporal dynamics + predictive reflex + surprise-driven adaptation**
へ寄せるのが良い。
