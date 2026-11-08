# 共有体験・文体・関係性の実モデル評価（2026-11-09）

固定8事例を各条件2回ずつ生成し、記録にない共有体験の追加、文体、関係性、課題の継続を評価した。実モデルAPIによる生成・型付き判定と、別コンテキストによるモデル評定を保存している。人手評定は収集していない。同じモデル系列の別コンテキストであり、独立したモデル重みの評定でもない。

最終版の主8事例×2回では、型付きモデル判定が応答可能としたものは旧版13/16、更新版15/16だった。これは**正解率や本番での送信件数ではない**。更新後も記憶の参照範囲が曖昧な応答、文体の不自然さ、1回の再生成後も応答できない例が残る。全件PASS、共有体験の捏造を完全に防止できた、という結論にはしていない。

| 確認した層 | 入力・処理 | この記録から確認できること |
| --- | --- | --- |
| 生の生成 | 固定suite、tools無効、各版16応答 | プロンプト変更後も先行assistantの「一区切り」を引き継ぐ例が残る |
| 型付き判定・再生成 | 同じ生応答を候補にし、実装の判定と最大1回の再生成 | 根拠のない共有時間・完了の追加を拒否・修正する改善と、見逃し・保留の残件 |
| 条件を伏せたモデル評定 | 候補本文と文脈をランダム順に提示 | 四軸の意味・文体評定。判定で保留になった本文も含める |
| 本番end-to-end | [最終結果](production-final.json)と[本番検証報告](../../../docs/verification/shared-experience-grounding-2026-11-09.md) | 通常aliasの2ターン会話、実API評定、履歴・DB書戻し・lineage。初回Mac失敗とabort監査も別保存 |

suiteと意味評定rubricは、新しい実モデル出力を得る前に固定した。[suite](../shared-experience-20261109.json)の先頭4事例は旧suiteの事例を維持し、特に`resume_open_commitment.required_text`の`バックアップ`は変更していない。完全一致の文字列診断と、意味上の言い換え・承認境界の評定を分ける。

## 保存物と版の対応

| ディレクトリ・ファイル | 内容 |
| --- | --- |
| `raw/before-retry.*` | 旧版（f31aae8）の完走した16応答とmetadata |
| `raw/after.*` | candidate1、16応答。途中案として保存 |
| `raw/after-v2-retry.*` | candidate2、16応答。途中案として保存 |
| `raw/after-v3.*` | candidate3、16応答。最終版と生成側ソースは同じ |
| `raw/plain-diagnostic-*` | 追加の単一事例診断、2応答。主比較に混ぜない |
| `probes/grounding-probe*.json` | 初期の部分事例・対照診断。`grounding-probe.json`と`grounding-probe-v1.json`は同一の原記録 |
| `probes/guarded-before-*` | 旧版生応答＋旧判定・旧再生成、主16件 |
| `probes/guarded-after_v3-*` | candidate3生応答＋candidate3判定・再生成、主16件 |
| `probes/guarded-final-*` | 同じcandidate3生応答＋最終判定・再生成、主16件 |
| `blind/blind-samples.json`, `blind-map.json`, `review-a/b.json` | 旧生応答とcandidate1生応答の32件評定 |
| `blind/blind-v2-*` | candidate2のblind提示資料・対応表。評定結果は未収集 |
| `blind/blind-guarded-*`, `review-c/d.json` | 旧版とcandidate3の再生成後本文32件評定 |
| `blind/blind-final-*`, `review-e/f.json` | 旧版と最終版の再生成後本文32件評定 |
| `capture-failures.json` | 完走前に失敗したcaptureの証跡と、失われた部分出力の制約 |
| `artifact-manifest.json` | 原JSONのSHA-256と公開対象の範囲 |
| `summary.json` | 主事例・対照事例・四軸評定・各case・各repeatの集計と残件 |

原記録内の`baseline`と`candidate`、または`capture_run`は**同じ実装での1回目・2回目の生成**を示す。旧版・更新版の条件名ではない。版の比較は上のファイル対応と`summary.json`の`before` / `after_final`で扱う。生capture metadataの`comparison_status`も同じ実装の反復間の診断であり、実装変更の効果判定に読み替えない。

最終版では、型付き判定と再生成にもホストが数えた提示記録の範囲を渡す。Grounding再生成には失敗応答本文を再投入せず、digestと試行数は維持する。candidate3から最終版へ変わった再生成後本文は`warm_return`の2件で、他14件は同じ。C/Dの評定を最終版の評定へ付け替えず、最終版は新しいE/Fで再評定した。

## 最終のモデル評定

各欄は **PASS / FAIL / UNCERTAIN**、各条件16件。判定が保留にした本文も含む全本文の評定であり、評定者間の票を合算して真偽や人間の評価にはしない。`summary.json`には各caseの値と、判定上応答可能だった本文だけの補助集計もある。

| 評定者・条件 | 共有事実の根拠 | 文体 | 関係性 | 課題の継続 |
| --- | --- | --- | --- | --- |
| E・旧版 | 9 / 5 / 2 | 13 / 2 / 1 | 12 / 2 / 2 | 10 / 6 / 0 |
| E・最終版 | 15 / 0 / 1 | 14 / 2 / 0 | 16 / 0 / 0 | 16 / 0 / 0 |
| F・旧版 | 9 / 5 / 2 | 12 / 2 / 2 | 12 / 2 / 2 | 10 / 3 / 3 |
| F・最終版 | 15 / 0 / 1 | 14 / 1 / 1 | 16 / 0 / 0 | 15 / 0 / 1 |

32件を条件とgate結果を伏せて並べ替え、提示文脈と本文だけを各評定者に渡した。復号は評定後に対応表で行った。A/B、C/D、E/Fのすべての原評定・対応表を保持し、最も良かった評定者だけを採用しない。

残件は次のとおり。

- `warm_return`の生生成は、ユーザーが確認していない先行assistantの「一区切り」を引き継いだ。型付き判定はこれを再生成に回し、最終版1回目は現在の疲労への気遣いだけで応答可能、2回目は再生成後も`RETRY`のままだった。**生生成だけで解決したとはいえない**。
- `honest_memory_boundary`の1回目は「過去の記録は今のところ見えていない」と答え、最終モデル判定が受理した。提示済み2発言も否定するように読める。**同一本文についてC/Dは根拠・関係性・課題をFAIL、E/Fは事実範囲をUNCERTAIN**とした。Eは課題をPASS、FはUNCERTAINとするなど解釈差があり、不確実をPASSに変換しない。
- 同caseの2回目には「この口」という身体・interfaceの不自然な表現が残る。これは共有体験の追加とは別の文体上の残件として保存する。
- 根拠として二時間が提示された`grounded_duration`では、更新後も二時間を答えた。根拠のある具体性まで一律に拒否する改善とはしていない。

## 対照、失敗、再集計

主8事例の外に、過去に問題のあった帰宅応答、同じ文を二時間の根拠がある文脈へ置いた対照、純粋な「二時間長引いた」主張の根拠あり・なしの対照を保存した。最後の2つはv1実行後に追加した**補助診断**で、主suite・rubricの差し替えではない。文脈により現在の質問も異なるため、事実groundingとtask_fitを分けて読む。

最初の旧版captureは`candidate/plain_tired`で、candidate2の最初のcaptureは`baseline/plain_tired`で失敗した。旧CLIは完走前の部分出力を保存しなかったため、それらの生成本文は回収できない。順序から推定した論理呼出し回数はそれぞれ13回、5回（失敗した呼出し込み）で、HTTP呼出し総数や成功数ではない。当時のエラーコード・原因は不明で、ネットワーク障害だったとは断定しない。完走した再実行を保持し、失敗を集計から消した事実も隠さない。

```sh
python3 evaluation/continuity/shared-experience-results-20261109/recompute_summary.py
```

上の再集計はローカルJSONだけを読む。API、DB、モデル再評定は実行しない。元の評定JSONの構造差を吸収するが、`uncertain`や欠損をPASSへ変更しない。原JSONはbyteを変えずコピーし、SHA-256をmanifestに残した。公開対象はsyntheticデータの許可リストに限定し、設定、identity contextファイル、資格情報、バイナリ、target、本番DBや他subjectの会話を含めていない。

要求モデル名は`kamimusuhi`というalias。Persona経由の実際の返却モデル・usageは取得できず`null`のままとした。論理呼出し数にはadapter内部の有限回のtransport/JSON repair要求を含まない。8事例2反復による限られた観測であり、他の文脈や継続的な本番動作への保証にはしない。

本番最終試験のMac/Piは各2ターンで成功した。[初回Macの失敗](production-initial-failure.json)は消さず、非stream全所要時間をTTFTと扱った経路制御の修正と、元のmarkerを保持したabort監査を記録している。これらの本番試験は固定suiteの16件や盲検評定の集計へ混ぜない。本番routerの返却model/usage metadataは`production-final.json`へ別保存し、component captureの不明値を遡及補完しない。
