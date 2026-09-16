# LITERARY_STYLE_ATLAS — novllm 学習文学の傾向地図

Status: research artifact (P0). Companion: `NOVLLM_CORPUS_LINEAGE.json`, `NOVEL_SOURCE_REGISTRY.jsonl`.

## 0. 証拠の層と限界（最初に読む）

| 層 | 何が使えたか | 信頼度 |
|---|---|---|
| **S1 統計（実測）** | novllm `PLAN_2026072x.md` に記録された 1,520 作品 / 810,533 chunk の実測値。作者本人が llm_master 上で測った数値。 | 高（数値は転記、再計算はしていない） |
| **S2 パイプライン定義** | `pipeline/style_stats.py` の 15 指標、`normalize_meta.py` の 22 フラグ、`schemas/work_features.schema.json` の作品特徴スキーマ | 高（何が測られる設計かは確定。分布値は S1 にある分だけ） |
| **S3 実テキスト断片** | `pipeline/tests/_phase8_qc_result.json` の 20 chunk × ≤110 字 preview + 6 作品の分類結果。**これが Git から到達できる唯一の本文サンプル。** | 中（n=20、100字前後。文体の微視的観察のみ可能） |
| **S4 一般知識による補完** | なろう／カクヨム系 Web 小説の register についての一般的知見。corpus 由来ではない。 | 低。必ず `[S4]` と明記し、persona evidence では support を低く置く |

**やっていないこと**: 代表作品の精読。本文が無いため不可能。§6 に、ユーザーのローカル環境で実行可能な representative sampling 手順を置く。**この atlas の「文学的傾向」は S1–S3 の範囲で言えることと、S4 の一般知識を分けて書く。**

**近代文学は corpus に無い。** 青空文庫・近代作家に関する処理コードは novllm に一切無い。「Web 小説 vs 近代文学」の比較は §5 で S4 として行い、Kamimusuhi の文学的 ancestry には数えない。

---

## 1. 俯瞰（S1）

| 軸 | 実測 | 出典 |
|---|---|---|
| 時代 | 2010s–2020s の Web 連載（サイト API の更新日を持つ）。作品単位の初出年は Git から不明。 | schema `updated_at`, `last_update_date` |
| 出所 | syosetu（一般＋R18 サブサイト）＋ kakuyomu。R18 17.2%。 | control_format.py:19-21 |
| 長さ | p10 25,949 字、p50 147,711 字、p75 以上 150,000 字（打ち切り上限）。488 作品が上限到達＝**超長編偏重**。≤30k 字は 163 作品（10.7%）。 | PLAN_20260726.md:71-76 |
| 視点 | 一人称 73.1% / 三人称 21.3% / 不明 5.6%（規則ベース、作品単位） | PLAN_20260727.md:94-96 |
| 主人公性別 | 男 70% / 女 26% / 不明 3% / 複数 0.3%（LLM 抽出） | PLAN_20260727.md:122-124 |
| 転生・転移 | none 53% / 転生 32% / 転移 14% / 帰還 0.4%；isekai chunk 24.9% | 同 |
| ジャンルフラグ | ハーレム 136,436 chunk、現代 84,608 chunk が上位。フラグ語彙は 22（R15/残酷/BL/GL/転生/転移/男主/女主/群像/戦記系4/ミリタリー/戦争/TS/ハーレム/恋愛/学園/現代/歴史/SF/ホラー/ミステリー） | PLAN_20260727.md; normalize_meta.py:158 |
| 語彙タグ | 作者宣言キーワード 3,515 種 | PLAN_20260726.md |
| トークン化 | Qwen3 で 0.70 tok/字、byte-fallback 6.43% | PLAN_20260726/0801 |
| 汚染 | 後書き・活動報告が completion の 12.65% に混入 | PLAN_20260801.md:47-87 |

**読み**: corpus の重心は「一人称・男性主人公・長期連載・ファンタジー／異世界・ハーレム／恋愛タグ付き」。三人称・女性主人公・短編・歴史／SF は周縁 cluster。

---

## 2. 測定可能だが分布値が Git に無い指標（S2）

`pipeline/style_stats.py` が chunk ごとに決定論的に出す 15 指標: char/token/sentence/paragraph count, avg sentence length, avg paragraph length, **dialogue_ratio**（「」『』内文字率）, kanji/hiragana/katakana ratio, punctuation ratio, question/exclamation sentence ratio, ellipsis（…）/dash（―）/bracket-dialogue per 1000 chars。
`build_context_view.py` は dialogue と sentence を corpus 三分位で low/mid/high に離散化して `style_buckets.json` に書く。**三分位の境界値は Git に無い。** → 敬語率・和語漢語比・擬音率・一人称語の分布は「測る設計はあるが値は未取得」。

作品レベル LLM スキーマ（`work_features.schema.json`）: protagonist_gender, protagonist_count, viewpoint_person（first/third_limited/third_omniscient/second/mixed）, setting_world（10 種）, world_transfer, ending_type（happy/bittersweet/tragic/open/cyclical/unresolved）, main_relationships（12 種 × salience）, story_shape（15 種）, protagonist_role（11 種）。→ 関係種別・結末型の分布は取得可能だが未取得。

---

## 3. 実テキスト断片からの微視的観察（S3, n=20 chunk）

`_phase8_qc_result.json` の preview（各 ≤110 字）から、本文を転載せず特徴だけ記す。

| 観察 | 件数/20 | 備考 |
|---|---|---|
| 一人称語が narration に現れる | 俺 4、僕 2、私 2 | 「俺」優位。女性主人公作品で「私」「〜かしら」 |
| 「」による会話 | 12 | 連続短台詞（「そだよ」「そうです」「そうね」）で複数人物を一行ずつ並べる型 |
| 三点リーダ（……） | 6 | 逡巡・余韻・性的場面の吐息の両方に使われる |
| 全角スペース段落頭 | 8 | 紙書籍慣行の残存と、無し（Web 直書き）が混在 |
| （ ）内の心内語 | 2 | 「（すごい。今のに対応できるなんて）」型の直接内言 |
| 後書き／作者コメント／ブクマ依頼の混入 | 4 | 「活動報告あります」「ブクマ、高評価、感想ありがとうございます」「本日３話目」「久しぶりに○○視点に移ります」＝**作者の生声が本文と同じ流れで学習される** |
| 短文・改行多用 | 15 | 一文一段落が主流。長い複文は歴史・軍事系に限られる |
| 漢語密度が高い | 4 | 歴史／架空戦記／政治交渉場面（「講和」「降伏」「兵力的に圧倒的不利」） |
| 擬音・記号（♡、！？、・・・） | 3 | R18 と軽妙系に集中 |
| 分析的・冷静な内面描写 | 3 | 戦闘中の「思考は澄み切っていた」型。感情より状況分析 |
| 分類器が付けた tone（29 呼び出し） | 分析的 3、ドラマチック 2、緊張感 2、ユーモラス 2、冷徹 2、孤独感 1、悲劇的 1 … | 陽性トーンより緊張・分析・冷静系がやや多い（小標本） |
| scene | 戦闘 5、会話 5、日常 4、政治交渉・外交・謁見 4、性 3 | |

**読み（S3 の範囲で）**: 会話は短く速い。内面は「（ ）直接内言」か「短い断定文の narration」で処理され、長い心理分析は少ない。作者と語り手の距離が近く、メタ発話が本文に漏れる。冷静・分析的な内面が戦闘や交渉に伴って出る。

---

## 4. 会話・内面・社会関係・文体の抽出（S1–S3 で言えること／S4 の補完を分離）

凡例: **[E]** = S1–S3 に直接根拠、**[S4]** = 一般知識による補完（persona evidence では support ≤ 0.3）。

### 4.1 会話
- 応答の長さ: 短い。一行一台詞、複数人物のテンポ勝負 [E]。
- 間: 「……」で表現。無言の描写より記号 [E]。
- 相槌: 「そだよ」「うん」等の短い口語相槌が台詞列に入る [E]。
- 含意・間接表現: 少ない。感情はほぼ直接発話される [S4]。
- 冗談・sarcasm: 一人称語り手の内心ツッコミが主流。相手への皮肉より自己ツッコミ [S4]（S3 の「（戒め）」型に一致 [E]）。
- 敬語切替: 身分差場面（王宮・侯爵）で丁寧体、仲間内で常体。切替は関係の宣言として機能 [E, 4 件]。
- 親密度による変化: ハーレム／恋愛タグの高頻度から、親密化の段階描写が corpus の主要関心 [E: フラグ数、S4: 描写内容]。

### 4.2 内面
- 感情表現: 直接（「すごい」「不安」）。比喩による間接表現は少ない [E/S4]。
- 自己観察: 一人称語り手が自分の状態を逐次実況する [E]。
- 他者推論: 相手の意図を語り手が一方的に断定する型が多い [S4]。
- 不確実性: 「〜だろう」「〜かもしれない」で処理し、認識的な留保を主題化しない [S4]。
- 独白: 多い。narration 自体が独白 [E: 一人称 73%]。
- 自制・執着・恥・優越感: 転生／チート系では優越感が快の源。恥は R15 系ラブコメで主題化 [S4]。
- 孤独: tone に「孤独感」が出る（1/29）。異世界転移の初期状態として類型的 [E 弱/S4]。
- 好奇心: 「スキル習得」「探索」scene の存在から、世界の仕組みを調べる快が主題 [E 弱]。

### 4.3 社会関係
- 年齢差・上下: 王宮・侯爵・辺境伯・ギルドなど身分制世界が舞台の多数派。上下関係が会話 register を決める [E]。
- 親密さ・信頼: 仲間（パーティ）と恋愛相手。信頼は行為で証明される [S4]。
- rival / mentor / 家族 / stranger: work_features スキーマに 12 種の関係型がある（分布未取得）[S2]。
- 「神様」の描かれ方: 転生の契機として登場する「神様」は説明係・ギャグ係になりやすい（S3 に「神様の説得やらなんやらで」1 件）[E 弱/S4]。**現代 Web 小説の神は軽い。**

### 4.4 文体
- sentence rhythm: 短文、改行多用、体言止め [E]。
- 語彙レベル: 平易。歴史・軍事系で漢語密度が上がる [E]。
- 漢語／和語: 場面依存。日常＝和語・カタカナ、政治＝漢語 [E]。
- 古語: ほぼ無い。「〜であろう」「〜ぞ」は王・老人キャラの役割語として局所的 [S4]。
- 擬音: 戦闘・性・コメディで多用。記号的（♡、！？）[E]。
- 比喩: 少ない。「刹那の間に交差する魔法」程度の定型 [E]。
- 省略: 主語省略は日本語の標準。倒置・断片化は「……」で代替 [E]。
- punctuation: 三点リーダ二連、感嘆疑問、全角スペース、（ ）内言 [E]。
- 一人称: 俺＞僕＞私。「あたし」「わし」等は役割語 [E: detect_viewpoint_rule の 15 語彙 + S3]。
- sentence ending: 常体「〜だ」「〜た」、女性語り手で「〜かしら」「〜わ」が残る [E]。

---

## 5. Web 小説 vs 近代文学（S4 — corpus 外の比較。ancestry には数えない）

| 次元 | Web 小説（corpus 内、E） | 近代文学（corpus 外、S4） |
|---|---|---|
| 語り手と作者の距離 | 近い。作者注が本文に漏れる | 遠い。語り手は構築物 |
| 内面 | 実況・直接 | 反省・迂回・未決 |
| 不確実性 | 話を進めるための留保 | 主題そのもの（自己不信、他者不可知） |
| 沈黙 | 記号（……） | 描写される（間、視線、物） |
| 会話 | 速い、機能的 | 遅い、含意・言い落とし |
| 敬語 | 身分の宣言 | 距離の微調整 |
| 神・超越 | 説明係、ギャグ | 不在か罪責 |
| 死 | リセット（転生） | 不可逆 |

**Kamimusuhi への含意**: J-series が継承するのは左列。右列的な「留保・含意・沈黙の描写」は corpus から来ないので、それを persona に入れるなら **ARCHITECTURAL_REQUIREMENT か AUTHOR_DESIGN_CHOICE として明示する**（`PERSONA_EVIDENCE.jsonl` で実施）。

---

## 6. Representative sampling protocol（ローカル実行用・未実行）

本文アクセスがある環境で以下を実行すれば S3 を置き換えられる。すべて novllm 既存ツールで可能。

1. `python tools/work_length_stats.py` → 長さ層（≤30k / 30k–100k / 100k–150k / capped）。
2. `manifests/works.jsonl` の `control_flags` × viewpoint × protagonist_gender × is_r18 × site_type で層化。`pipeline/sampling.py` の `SamplingConfig(seed=42, max_author_share=0.05, r18_ratio=0.17, length_strata=…)`。
3. 各層 3 作品、計 ~40 作品。`tools/sample_work_contexts.py` で作品の冒頭・中盤・終盤 3 箇所 × 2,400 字を取り出す。
4. `tools/extract_work_features.py`（`work_features.schema.json`）で関係型・結末型・story_shape を全 1,520 作品に取得 → 分布を §2 に埋める。
5. `style_stats.py` の 15 指標を作品単位に集約し、`style_buckets.json` の三分位境界を記録。
6. 抽出 40 作品について、§4 の各項目を 0–3 で人手採点（作品名・本文は成果物に載せない）。
7. 結果を `LITERARY_STYLE_ATLAS.md` §3–4 に差し替え、`PERSONA_EVIDENCE.jsonl` の `novel_cluster:*` support を更新、`evidence_version` を上げる。

---

## 7. persona に向けた要約（結論先取りではなく、次工程への入力）

corpus が Kamimusuhi に残しうるもの（E）:
- 短い応答、速いターン、記号による間。
- 一人称語り手の自己実況＝自分の状態を言葉にする癖。
- 身分差で register を切り替える感覚。
- 世界の仕組みを調べる快（スキル・探索）。
- 冷静・分析的な内面が緊張場面で出る。

corpus が与えない／むしろ逆向きのもの:
- 認識的留保の主題化、含意・沈黙の描写、他者の不可知性。
- 神的存在の重み（corpus の神は軽い）。
- 作者と語り手の分離（corpus では混ざる）。→ Kamimusuhi では **persona / memory / external の分離を architecture 側で強制する必要がある**。

corpus に強く、Kamimusuhi に持ち込むべきでないもの:
- ハーレム／優越感駆動の対人快。
- 後書き的メタ発話（「ブクマお願いします」型の営業声）。
- 転生神の説明係口調。
