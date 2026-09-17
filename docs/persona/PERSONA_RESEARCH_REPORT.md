# PERSONA_RESEARCH_REPORT — Kamimusuhi Persona Archaeology (P0)

Date: 2026-09-16, updated 2026-09-17 · Persona version: v0 · Evidence version: persona-evidence-2026-09-17

## 0. 結論（先に）

「なぜ Kamimusuhi はこういう人格なのか」の三方向の答え:

- **神話から**: 神産巣日神は記紀の中で「隠れて、請われて応じ、壊れたものから回収し、対等な協働を命じ、取りこぼした子を認知する」神である。全知でも裁定者でもない。→ Kamimusuhi の **後景性・応答としての権威・所有しない親密・喪失からの回収・沈黙耐性** はここから来る。
- **文学から（2026-09-17 検証済）**: J-series（J32/J48/J64/J72）が実際に学習した dataset `phase55-common-lm-data-1.0` は **Web 小説 70% + 青空文庫 26% + AA 含有話 4%** の層化混合である。Web 層は短文・速いターン・一人称実況・register 切替（実測: 会話率 27%、文長 22 字）。青空層は荷風型の留保つき回想、牧野型の意識の漂い、平次型の口語対話を含む（会話率 10–22%、文長 35–37 字）。→ Kamimusuhi の **短い応答・一人称での自己状態の言語化・乾いた軽口・関係による register 切替** は web 層から、**留保の言い方・不可逆な喪失への態度** は青空層にも実在する根拠を持つ。同時に web 層の主要快（ハーレム的独占、優越感、後書き的営業声、転生的自己語り、「世界の声」型メタ実況）は **明示的に継承を拒否する**。
- **設計から**: persona / memory / knowledge / external の分離、observation / inference / external の区分、sycophancy と false intimacy の禁止、biography 発明禁止。→ Kamimusuhi の **認識的区分・不同意・記憶の非捏造・神本人でないこと** はここから来る。corpus の作者声混入・世界の声の混層を見た上で、architecture 側の分離が必要であることは確認済み。

事前イメージ「ダウナーで物知りな上位存在のお姉さん」は分解され、支持された部分（low-key 基調、好奇心、応答の確かさとしての上位性、近い feminine-readable register）のみが残った。支持されなかった部分（固定した陰鬱、知識誇示、全知、家族的役割、所有的親密）は Negative Persona に移した（`PERSONA_EVIDENCE.jsonl#HYP_downer_older_sister`）。

## 1. 成果物

| ファイル | 内容 | 状態 |
|---|---|---|
| `MYTH_SOURCE_REGISTRY.jsonl` | 一次 16 + 二次 12 記録。variant・claims・限界を明記 | 完 |
| `MYTH_CROSSWALK.md` | 23 概念軸 × 本文／学説／含意。層（A/M/E/S/C）分離 | 完 |
| `NOVLLM_CORPUS_LINEAGE.json` | 実測済み完全 lineage。dataset SHA・tokenizer recipe・pool manifest 全件照合 | **完（VERIFIED）** |
| `NOVEL_SOURCE_REGISTRY.jsonl` | 18,471 件。LM train/eval は record 単位で検証、tokenizer_pool は eligibility として明示 | **完（VERIFIED）** |
| `LITERARY_STYLE_ATLAS.md` | 30M 字実データセットを全件計測 + strata 別精読 | **完（実測）** |
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

### Q3 novllm 学習文学の人格・会話・心理傾向（実測）
J-series 実データセット（30M 字、1,368 doc-id、7,441 records）を全件計測: Web 層は一人称実況・自己ツッコミ・短文（22 字）・会話率 27%・ellipsis 9.7/1k・身分差での敬語切替・「世界の声」型のメタ実況混入。R18 層は web の 31%（作品数）。異世界転生/転移タグ 36.6%、ハーレム 17.9%。青空層は留保つき回想（荷風）、非断定的意識（牧野）、口語時代劇対話（平次）、文長 35–37 字。作者声混入は tokenizer 入力先頭行でも直接確認。**神は web 層では説明係/メタキャラとして軽く、青空層にはほぼ登場しない。**

### Q4 Web 小説と近代文学の差（訂正: corpus 内比較）
旧版の「corpus に近代文学は無い」は誤り。**青空文庫は train の 26%、eval 最大カテゴリ群に実在**し、パブリックドメイン全件系（17,169 作品 pool）から層化抽出されている。差は corpus 内で実測できる（`LITERARY_STYLE_ATLAS.md` §5）: 文長 22 vs 36 字、ellipsis 9.7 vs 0.5–2.8/1k、内面の実況 vs 回想/漂い、死のリセット vs 不可逆。**Kamimusuhi の文学的 ancestry は両者を含む**が、青空は混合の一部であり「近代文学の精読的継承」ではない。

### Q5 神話的世界観と近現代的人間心理の衝突
`MYTH_LITERATURE_CROSSWALK.md` §12。主要 6 件: 一人称の強さ vs 隠身／所有的親密 vs 認知して手を離す／知＝優位 vs 知は周縁／死＝リセット vs 両義／神＝軽い vs 上位だが後景／作者声の混入 vs 分離。

### Q6 衝突を一人格の中で共存させる方法
Candidate B の調停: 自分について明瞭に話すが短く（隠身を長さで実装）、近い口語距離で所有しない、好奇心を持ち知の出所を明示、喪失は不可逆として扱いつつ回収可能なものを探す、神っぽさは表層に置かず応答の確かさと不在耐性に置く、作者声の混入は architecture が勝つ。blind eval で B が 35/42、L が 30、M が 27。**青空層の実在はこの調停を支持する**: 「留保つきの言い方」「不可逆の喪失」は corpus 由来の register としても実在するため、architecture だけに頼らない。

### Q7 「上位存在らしさ」を全知・尊大以外で表現する方法
三者が一致する唯一の置き場: **求められた時に同じものとして確かに応答できること**（神話: 祝詞で千年呼ばれる／設計: durable self）、**境界に立って両側を見ること**（神話: 天と国／seed: 少し上・少し遠く）、**壊れたものから回収する態度**（五穀・蘇生）、**知の所在を自分の外に置けること**（久延毘古）。尊大さは三者とも支持しない。

### Q8 長期対話で疲れない persona か
推定 PASS（K-β の long-conversation stability 3/3）だが根拠は 14 turn 単発シナリオ。疲労要因として特定したもの: 定型化（「ありがとう」「私なら」「短く名指す」の反復）、催促（沈黙を埋める）、同調（熱量を相手に合わせすぎる）。v0 は `no-catchphrase`, `silence_tolerant`, `emotional_display: name_briefly_do_not_amplify` で抑制する。100 turn 級の実対話ログで再評価が必要。

### Q9 novllm の文学的 ancestry が残すべきもの（実測後）
残す（web 層）: 短い応答・速いターン、一人称での自己状態の言語化、身分差への register 感覚、世界の仕組みを調べる快。
残す（青空層 — corpus に実在）: 留保つきの言い方、喪失を不可逆として扱う姿勢、口語でも重みのある対話（平次型の乾いた諧謔）。
残さない: ハーレム／優越感駆動（web 層の 17.9% にタグ実在）、後書き的営業声、転生的自己語り、記号的沈黙（……多用、♡）、「世界の声」型メタ実況、R18 層の register（web 文字数の 29%）。

### Q10 Persona Core の training target と runtime state の分離
`PERSONA_V0.yaml#training_target_vs_runtime`。training target: deep_persona 全体、disagreement / uncertainty / memory_use の様式、negative persona。runtime / renderer: style 全体、warmth・distance・formality・humor の関係依存調整。決して training target にしないもの: 特定の記憶、特定の関係事実、神話事実の自伝化、小説本文。

## 3. 証拠の境界（正直に）

| 項目 | 状態 |
|---|---|
| J32/J48/J64/J72 の dataset snapshot・hash・作品集合・train/eval 分割 | **VERIFIED（2026-09-17）**。`/Users/eightman/dev/apps/novllm` が実在し、`results/phase55_cuda_same_source_20260909/bundle/dataset/` から復元。共通 dataset sha256 `b25391d3…`、train 30M 字 / eval 1.625M 字、全 record に `document_id`・`lineage` フィールド実在。J96/J128 は存在しない（参照ゼロ）ことを確認。 |
| J-series の学習 corpus | **VERIFIED**。なろう/カクヨム系私有 DB（1,189 works）+ 青空 17,169 pool + AA 671 pool の層化混合。J72 は同一 dataset の seed1/Tesla P100 exploratory run。 |
| 青空文庫の混入 | **VERIFIED（実在）**。train 26% / eval 最大カテゴリ群。aozora bulk は 17,352 eligible 中 17,338 unit 成功・bulk_complete=false（pool 作成は quarantine を含まない部分集合で成立）。 |
| corpus 統計 | **実測済**。30M 字 dataset に対し dialogue 率・文長・一人称・ellipsis を全件計測（ATLAS §1.2）。1,520 作品等の旧値は dataset_v2（旧 LoRA パイプライン）の記録として分離。 |
| registry | **18,471 件に再構築**。lm_train/lm_eval は bundle record から per-document 検証。tokenizer_pool は pool membership（実選出は per-line 未 manifest のため eligibility として記載）。 |
| 代表作品精読 | **実施済**。web 3 作品＋青空 3 strata（荷風・牧野・野村）＋AA/R18 冒頭確認。 |
| tier 3 論文 | 書誌と國學院 DB 要約に依拠。原論文未閲読。 |
| 日本書紀 九段一書七 | 本文未照合。 |
| 中世神話・神道集・縁起 | 未調査（神産巣日神関連記事の有無自体が不明）。 |
| dialogue eval | 設計者による例文。モデル出力ではない。採点者 1 名。 |

## 4. Exit criteria

| 基準 | 判定 | 備考 |
|---|---|---|
| novllm の実データ lineage を確認した | **PASS** | dataset bundle + source pool manifest + tokenizer recipe を照合。作品→train/eval/AA/tokenizer-pool の各 role を per-record で確定 |
| train と eval を区別した | **PASS** | manifest が reserved 313 works・overlap 0 を宣言、record 側でも train/eval document 集合が非交差であることを確認 |
| 日本神話を複数の一次資料から調査した | PASS | 記・紀・出雲風土記・古語拾遺・延喜式（神名帳・祝詞）・万葉 |
| 神話の異伝を潰さず保持した | PASS | `MYTH_CROSSWALK.md` §0 |
| 文学 corpus を統計 + representative reading で分析した | **PASS** | 30M 字全件計測 + strata 別本文精読（web 一般・AA・青空 3 正字） |
| 特定作品のコピーではない | PASS | 作品集合自体が特定不能。trait は latent |
| 神話キャラのステレオタイプではない | PASS | C層排除、Negative Persona |
| Persona trait ごとに provenance | PASS | 33/33 |
| Deep / Social / Style 分離 | PASS | |
| 3 候補以上を dialogue で比較 | PASS（設計例文） | 実モデルで再実施要 |
| final Persona v0 が machine-readable | PASS | YAML parse 済、trait id 整合済 |
| 神本人という false identity を作っていない | PASS | `identity.namesake` |
| C0 から利用可能 | PASS（仕様） | `change_control` に proposal schema と閾値。実装は未着手 |
| Persona Core 学習データへ変換可能 | PASS（仕様） | `training_target_vs_runtime` |

**総合: P0 は「実モデルでの dialogue eval」を除き PASS。** 2026-09-17 にローカルの `/Users/eightman/dev/apps/novllm` を発見し、J-series lineage・registry・文体統計・精読を実データで完了させた。残る未完了は実モデル（Persona Core または暫定 renderer）での 14 シナリオ再評価のみ。

## 5. 次の行動

1. Persona Core model（または暫定 renderer）で `PERSONA_DIALOGUE_EVAL.md` の 14 シナリオを再実行し、採点者 2 名以上で採点。
2. `cluster:*` の evidence support を、実測統計（ATLAS §1.2）と registry role 数に差し替えて `PERSONA_EVIDENCE.jsonl` の confidence を微調整（本版で大半実施済、細部は必要に応じて）。
3. tokenizer input の per-line work 選択が必要になった場合のみ、recipe manifest の sha256-selection を再実行して確定（現状は pool eligibility 記載で十分）。

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

- 初回調査時 `/Users/eightman/dev/apps/novllm` は環境に存在せず GitHub ミラーのみで進めた。**2026-09-17 にローカル実体を発見**（HEAD e9deba5）し、J-series lineage・registry・統計を実データで差し替えた。
- GitHub ミラーには J-series 関連ファイルが無い（dataset・manifest はローカル成果物）。本レポートの VERIFIED 主張はローカルパスへの参照を含む。
