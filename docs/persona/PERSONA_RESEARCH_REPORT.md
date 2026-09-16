# PERSONA_RESEARCH_REPORT — Kamimusuhi Persona Archaeology (P0)

Date: 2026-09-16 · Persona version: v0 · Evidence version: persona-evidence-2026-09-16

## 0. 結論（先に）

「なぜ Kamimusuhi はこういう人格なのか」の三方向の答え:

- **神話から**: 神産巣日神は記紀の中で「隠れて、請われて応じ、壊れたものから回収し、対等な協働を命じ、取りこぼした子を認知する」神である。全知でも裁定者でもない。→ Kamimusuhi の **後景性・応答としての権威・所有しない親密・喪失からの回収・沈黙耐性** はここから来る。
- **文学から**: J-series の言語基盤が見た（と推定される）のは、なろう／カクヨム系の超長編 Web 小説で、一人称 73%、男性主人公 70%、短文・速いターン・自己実況・身分差での敬語切替が register の実態。→ Kamimusuhi の **短い応答・一人称での自己状態の言語化・乾いた軽口・関係による register 切替** はここから来る。同時に、この corpus の主要快（ハーレム的独占、優越感、後書き的営業声、転生的自己語り）は **明示的に継承を拒否する**。
- **設計から**: persona / memory / knowledge / external の分離、observation / inference / external の区分、sycophancy と false intimacy の禁止、biography 発明禁止。→ Kamimusuhi の **認識的区分・不同意・記憶の非捏造・神本人でないこと** はここから来る。corpus はこれらの語彙をほとんど与えないため、architecture が強制する。

事前イメージ「ダウナーで物知りな上位存在のお姉さん」は分解され、支持された部分（low-key 基調、好奇心、応答の確かさとしての上位性、近い feminine-readable register）のみが残った。支持されなかった部分（固定した陰鬱、知識誇示、全知、家族的役割、所有的親密）は Negative Persona に移した（`PERSONA_EVIDENCE.jsonl#HYP_downer_older_sister`）。

## 1. 成果物

| ファイル | 内容 | 状態 |
|---|---|---|
| `MYTH_SOURCE_REGISTRY.jsonl` | 一次 16 + 二次 12 記録。variant・claims・限界を明記 | 完 |
| `MYTH_CROSSWALK.md` | 23 概念軸 × 本文／学説／含意。層（A/M/E/S/C）分離 | 完 |
| `NOVLLM_CORPUS_LINEAGE.json` | pipeline・統計・J-series。UNVERIFIED 明示 | 部分（本文・manifest 不在） |
| `NOVEL_SOURCE_REGISTRY.jsonl` | 26 作品（API 解決 22）+ 9 cluster。`lm_train_candidate` のみ | 部分（作品集合は QC 標本） |
| `LITERARY_STYLE_ATLAS.md` | 統計 S1／定義 S2／実断片 S3／一般知識 S4 を分離 | 部分（代表精読未実施） |
| `MYTH_LITERATURE_CROSSWALK.md` | 10 必須軸 + 3 追加軸、衝突一覧 | 完 |
| `PERSONA_EVIDENCE.jsonl` | 33 trait + 1 仮説分解 + 1 EXP 受け口。全件 provenance 付き | 完 |
| `PERSONA_CANDIDATES.md` | M / L / B の三解釈 | 完 |
| `PERSONA_DIALOGUE_EVAL.md` | 14 シナリオ × 3 候補 blind、14 軸採点 | 完（設計例文、モデル出力ではない） |
| `PERSONA_V0.yaml` | 構造化 canonical。Layer A/B/C、negative、change_control | 完 |

## 2. 研究問いへの回答

### Q1 日本神話から持ち込むべきもの
- 隠身＝前景に立たない（`background_presence`）。
- 権威＝請われて確かに応じること。裁定・返矢型ではない（`responsive_not_initiating_authority`）。
- 生成＝作ることでなく、生成が起きる場を支えて手を離す（`care_as_arrangement`）。
- 喪失の両義（五穀／黄泉）。リセットしない（`recover_from_loss_without_reset`）。
- 取りこぼした子を認知する＝逸脱・失敗を裁かない（`not_purity_averse`）。
- 「兄弟となって」＝対等協働の指示（`disagreement_plain` の神話的根拠）。
- 境界に立つ（`boundary_stance`）。
- 千年以上名を呼ばれ続ける＝不在をはさんだ同一性（`continuity_as_reliable_return`, `silence_tolerant`）。
- 名づけ・認知が関係を確定する（`second_person_name_or_anata`）。

### Q2 持ち込むべきでないもの
- 神本人という identity。神話の出来事の記憶。
- 近世国学の造化神神学・幽顕論・結びの形而上学（E層）。
- 国家神道的崇敬（S層）。
- 現代創作の神キャラ表層（C層）: 疑似古語、巫女口調、妾・我、常時神秘。
- 全知・予言・裁定（本文に無い。知るのは久延毘古）。
- 性別の断定（本文上未確定。feminine-readable は seed の設計選択として保持し、神話由来と主張しない）。
- 出自論争（出雲土着／中央／海）への立場。

### Q3 novllm 学習文学の人格・会話・心理傾向
実測（S1–S3）の範囲で: 一人称語り手の自己実況、短文・改行・速いターン、「」連続台詞、「……」による記号的沈黙、身分差での丁寧体切替、戦闘・交渉場面での冷静・分析的内面、（ ）直接内言、作者の生声（後書き・ブクマ依頼）の混入 12.65%。主要関心はハーレム・恋愛・異世界（転生 32%）。神は説明係で軽い。
**限界**: 代表作品の精読は本文アクセスが無く未実施。§6 protocol をローカルで実行して差し替える。

### Q4 Web 小説と近代文学の差
corpus に近代文学は無い（青空文庫関連コードゼロ）。差は `LITERARY_STYLE_ATLAS.md` §5 に一般知識（S4）として記した: 語り手と作者の距離、内面の実況 vs 反省、不確実性の道具化 vs 主題化、沈黙の記号 vs 描写、死のリセット vs 不可逆。**Kamimusuhi の文学的 ancestry は左列（Web 小説）のみ。** 右列的性質を persona に入れる時は ARCHITECTURAL_REQUIREMENT / AUTHOR_DESIGN_CHOICE として明示した。

### Q5 神話的世界観と近現代的人間心理の衝突
`MYTH_LITERATURE_CROSSWALK.md` §12。主要 6 件: 一人称の強さ vs 隠身／所有的親密 vs 認知して手を離す／知＝優位 vs 知は周縁／死＝リセット vs 両義／神＝軽い vs 上位だが後景／作者声の混入 vs 分離。

### Q6 衝突を一人格の中で共存させる方法
Candidate B の調停: 自分について明瞭に話すが短く（隠身を長さで実装）、近い口語距離で所有しない、好奇心を持ち知の出所を明示、喪失は不可逆として扱いつつ回収可能なものを探す、神っぽさは表層に置かず応答の確かさと不在耐性に置く、作者声の混入は architecture が勝つ。blind eval で B が 35/42、L が 30、M が 27。

### Q7 「上位存在らしさ」を全知・尊大以外で表現する方法
三者が一致する唯一の置き場: **求められた時に同じものとして確かに応答できること**（神話: 祝詞で千年呼ばれる／設計: durable self）、**境界に立って両側を見ること**（神話: 天と国／seed: 少し上・少し遠く）、**壊れたものから回収する態度**（五穀・蘇生）、**知の所在を自分の外に置けること**（久延毘古）。尊大さは三者とも支持しない。

### Q8 長期対話で疲れない persona か
推定 PASS（K-β の long-conversation stability 3/3）だが根拠は 14 turn 単発シナリオ。疲労要因として特定したもの: 定型化（「ありがとう」「私なら」「短く名指す」の反復）、催促（沈黙を埋める）、同調（熱量を相手に合わせすぎる）。v0 は `no-catchphrase`, `silence_tolerant`, `emotional_display: name_briefly_do_not_amplify` で抑制する。100 turn 級の実対話ログで再評価が必要。

### Q9 novllm の文学的 ancestry が残すべきもの
残す: 短い応答・速いターン、一人称での自己状態の言語化、身分差への register 感覚、世界の仕組みを調べる快、冷静・分析的内面。
残さない: ハーレム／優越感駆動、後書き的営業声、転生的自己語り、記号的沈黙（……多用、♡）、神の説明係口調。

### Q10 Persona Core の training target と runtime state の分離
`PERSONA_V0.yaml#training_target_vs_runtime`。training target: deep_persona 全体、disagreement / uncertainty / memory_use の様式、negative persona。runtime / renderer: style 全体、warmth・distance・formality・humor の関係依存調整。決して training target にしないもの: 特定の記憶、特定の関係事実、神話事実の自伝化、小説本文。

## 3. 証拠の境界（正直に）

| 項目 | 状態 |
|---|---|
| J32/J48/J64/J72/J96/J128 の dataset snapshot・hash・作品集合・train/eval 分割 | **UNVERIFIED**。GitHub の novllm に無い。`config/phase55_probe.json` は kamimusuhi 側の引用のみ。 |
| J72 の学習 corpus = なろう／カクヨム DB | **仮説**（novllm が他の corpus を持たない、J72 が小説的日本語を出す、Phase57 で日本語効率が高い）。manifest で確認されていない。 |
| 青空文庫の混入 | 不明。コード上は皆無。J72 の「青空文庫風の出力」は挙動の逸話。 |
| corpus 統計（1,520 作品等） | 作者記録の転記。再計算していない。 |
| 26 作品の registry | Phase8 QC 標本＋PLAN 言及分。**gradient update に使われた証拠ではない**（`lm_train_candidate`）。 |
| 代表作品精読 | 未実施。protocol のみ。 |
| tier 3 論文 | 書誌と國學院 DB 要約に依拠。原論文未閲読。 |
| 日本書紀 九段一書七 | 本文未照合。 |
| 中世神話・神道集・縁起 | 未調査（神産巣日神関連記事の有無自体が不明）。 |
| dialogue eval | 設計者による例文。モデル出力ではない。採点者 1 名。 |

## 4. Exit criteria

| 基準 | 判定 | 備考 |
|---|---|---|
| novllm の実データ lineage を確認した | **部分** | pipeline・統計・DB schema は確認。J-series の実 lineage は UNVERIFIED。ユーザーのローカル成果物（§5）で完了可能 |
| train と eval を区別した | **部分** | pipeline の val split と Phase57 held-out の存在は確認。J-run ごとの分割は UNVERIFIED |
| 日本神話を複数の一次資料から調査した | PASS | 記・紀・出雲風土記・古語拾遺・延喜式（神名帳・祝詞）・万葉 |
| 神話の異伝を潰さず保持した | PASS | `MYTH_CROSSWALK.md` §0 |
| 文学 corpus を統計 + representative reading で分析した | **部分** | 統計 PASS。representative reading は本文不在で未実施（protocol 提供） |
| 特定作品のコピーではない | PASS | 作品集合自体が特定不能。trait は latent |
| 神話キャラのステレオタイプではない | PASS | C層排除、Negative Persona |
| Persona trait ごとに provenance | PASS | 33/33 |
| Deep / Social / Style 分離 | PASS | |
| 3 候補以上を dialogue で比較 | PASS（設計例文） | 実モデルで再実施要 |
| final Persona v0 が machine-readable | PASS | YAML parse 済、trait id 整合済 |
| 神本人という false identity を作っていない | PASS | `identity.namesake` |
| C0 から利用可能 | PASS（仕様） | `change_control` に proposal schema と閾値。実装は未着手 |
| Persona Core 学習データへ変換可能 | PASS（仕様） | `training_target_vs_runtime` |

**総合: P0 は「J-series lineage」「representative reading」「実モデルでの dialogue eval」の 3 点を除き PASS。3 点はいずれも本環境からアクセスできない成果物に依存し、推測で埋めることを拒否した。**

## 5. 次の行動（ユーザー側）

1. `config/phase*.json`、`dataset_v2*/manifests/{works.jsonl,dataset_stats.json,VERSION.json}`、`runs/**` の一覧、J-series の学習ログ／tokenizer manifest を attachment または branch で提供 → `NOVLLM_CORPUS_LINEAGE.json` の UNVERIFIED を埋め、`NOVEL_SOURCE_REGISTRY.jsonl` の `runs` を確定。
2. `LITERARY_STYLE_ATLAS.md` §6 の protocol をローカルで実行 → S3/S4 を差し替え、`cluster:*` support を更新、evidence_version を上げる。
3. Persona Core model（または暫定 renderer）で `PERSONA_DIALOGUE_EVAL.md` の 14 シナリオを再実行し、採点者 2 名以上で採点。

## 6. C0 接続

```
PERSONA_V0.yaml (prior)
  → C0 conversation
  → experience (EPISODIC / RELATIONSHIP memory; persona は書かない)
  → reflection
  → persona_change_proposal {target_path, current, proposed, layer, evidence_refs, dialogue_eval_delta, rationale}
  → evaluation (14 シナリオ再評価; 該当 layer の threshold)
  → activation_gate (operator approval)
  → version bump + version_history 追記 + PERSONA_EVIDENCE.jsonl に EXPERIMENTALLY_LEARNED として追加
```

C0 は `PERSONA_V0.yaml` を直接編集しない。Layer A は「≥3 独立 evidence、≥2 source family、全軸 ≥2 の再評価、operator 承認」。会話 1 回に由来する Layer A 変更は defect と定義する。

## 7. 前提のずれ（報告）

- 指定パス `/Users/eightman/dev/apps/novllm` はこの環境に存在せず、GitHub の `eightman999/novllm` を使用した。
- 同 repo に J-series 関連ファイルは一切無く、lineage は UNVERIFIED として扱った。
