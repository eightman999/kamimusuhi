# Recall hot 起点の熱減衰（`recall_hot_use_gain`）

`docs/implementation/recall-v0.md` の hot 起点（最近「使った」記録を宣言リンク・タグ想起の
起点にする仕組み）に、使用回数が効く熱減衰を**選択式**で足したもの。
別システム FIO の体験ログの「熱」（`live_heat = heat × exp(−dt / S)`、
`S = S_BASE × (1 + ln(1 + 参照回数))`）を移植元とする。実装済みの内容のみを記述する。

## 実装済みの内容

### 既定（`recall_hot_use_gain = 0.0`）— 従来どおり

`state_record_usage.last_used_at` からの経過 `age` が `recall_hot_window_secs`（既定 72 時間）
以内の記録が hot 起点になり、強さは窓の中で 1.0 → 0.5 へ直線的に下がる。窓の外はゼロ。
`use_count` は効かない。**既定値では挙動は変わらない**。

### 熱モデル（`recall_hot_use_gain > 0`）

```text
uses   = clamp(use_count, 1, HOT_USE_CAP)          # HOT_USE_CAP = 20
m      = 1 + gain × ln(uses)                       # 1 回だけ使った記録は m = 1
heat   = HOT_HEAT_FLOOR ^ (age / (window × m))     # HOT_HEAT_FLOOR = 0.05
       = exp(−age / S),  S = window × m / ln(1 / HOT_HEAT_FLOOR)
起点になる条件: heat ≥ HOT_HEAT_FLOOR（⇔ age ≤ window × m）
起点の強さ   : heat
```

- 1 回だけ使った記録は、従来の窓とちょうど同じ時刻で床に達して外れる（境界は含む）。
  よって熱モデルが従来より起点を**減らす**ことはない（強さの形は変わる）。
- よく使った記録ほど長く残るが、伸びは対数で、上限は窓の `1 + gain × ln 20` 倍
  （gain=1 で約 4.0 倍、gain=2 で約 7.0 倍）。
- `age` が負（時計の巻き戻り）は「いま使った」扱いで強さ 1.0。
- `recall_hot_window_secs = 0` は gain に関係なく hot 起点を無効にする。
- 語彙一致の起点（強さ 1.0）と重なる記録は従来どおり強い方を採る。

### 凍結を避けるための上限

FIO では「注入されただけで温まる」作りが、一度載った記憶が冷えずに居座る凍結を起こした。
ここでは

1. 温める（`use_count` を増やし `last_used_at` を更新する）のは応答・tool 引数/結果に
   記録 id が明示的に現れた「使用」だけ（recall-v0 の規則のまま、本変更では触っていない）、
2. 使用回数の効きは対数、
3. `HOT_USE_CAP = 20` で頭打ち

なので、どれだけ使われた記録でも最終使用から `window × (1 + gain × ln 20)` を過ぎれば
必ず起点から外れる。

## 実装の所在

| 層 | 場所 | 内容 |
|---|---|---|
| core | `crates/kamimusuhi-core/src/c0.rs` | `RetrievalParams::recall_hot_use_gain`（f64・既定 0.0・範囲 0.0–2.0）。`PARAM_BOUNDS`/`get`/`set`、float 扱い |
| runtime | `crates/kamimusuhi-runtime/src/dialogue_recall_links.rs` | 定数 `HOT_USE_CAP`/`HOT_HEAT_FLOOR`、純関数 `hot_seed_strength(age_ms, use_count, window_ms, gain) -> Option<f64>`、`select_declared` の hot 起点計算をこの関数に置換 |

## 検証

- `dialogue_recall_links.rs` 内 unit test 3 件:
  - `zero_gain_hot_strength_is_the_linear_window` — gain 0.0 で従来式と数値一致・窓外ゼロ・窓 0 で無効
  - `heat_model_stretches_with_use_up_to_the_cap` — 1 回使用は窓ちょうどで床・1 ms 超で外れる、
    回数単調性、上限超は上限と同値、`1 + gain × ln 20` 倍超は必ず外れる、負の経過は 1.0
  - `heat_model_keeps_a_much_used_record_hot_past_the_window` — `select_declared` 経由で、
    窓の 1.5 倍前に 12 回使われた記録が gain 1.0 で起点になりリンク先が候補に出る／gain 0.0 では出ない
- `c0.rs` 内 unit test 1 件: `recall_hot_use_gain_is_a_bounded_float_in_the_closed_vocabulary`
  （閉じた語彙への登録・範囲外拒否・未知キー拒否・小数の get/set）
- 既存テストは書き換えていない。`cargo test --workspace`（`HAI_API_KEY` 未設定）全緑。

## 限界・未実装

- **実データでは未測定**。gain をいくつにすべきか、熱モデルで想起の質が上がるかは未評価。
  `recall-eval` は hot/used 起点を履歴非再現として除外している（`recall_hot_window_secs` を 0 に固定）ため、
  この変更の効果も測れない。
- 既定 0.0 のため、明示的に param を変えない限り本番挙動は変わらない。
- 熱モデルでは窓の終わり付近の起点強さが従来（最低 0.5）より低く（最低 0.05）なる。
  hop 減衰と掛け合わさるので、起点にはなっても予算内の順位で落ちることがある。
- 熱は保存しない（`last_used_at`/`use_count` から都度計算する）。FIO の「熱そのものを持つ」形は取っていない。
