# K0-E2 — 能動的な情報取得

K0-E2 は、小型の非言語 Core が部分観測の環境で行動し、その結果として得た情報を次の判断へ使う実験です。Core は **continuous non-language controller**、J72 は **conditionally accessed language organ** として扱います。`INVOKE_LANGUAGE` は、費用を払って情報を取得する認知操作です。

実験条件の正典は [PROTOCOL.md](PROTOCOL.md)、環境 API は [ENV_CONTRACT.md](ENV_CONTRACT.md) です。実測結果・成功条件の判定・失敗例は [K0_E2_REPORT.md](artifacts/primary/K0_E2_REPORT.md) に集約します。集計が終わるまでは、実装やテストの完了から研究上の成功を推定しません。

```text
16個の数値観測 → 非言語 Core → 行動
                              ├ 注意・追加観測 → 改善された観測
                              ├ 想起         → 過去に観測した情報
                              ├ 待機         → 時間経過・資源回復
                              └ 言語呼び出し → 外部記録 → J72 → 厳密なparser
                                                            ↓
                                              数値の取得情報 → Core → 最終判断
```

## 隔離と構成

既存の [K0-E](../k0_brainstem/README.md) のコード・checkpoint・結果は immutable baseline として保存します。E2 は既存のモデル定義と PPO の GAE 計算を読み込みますが、baseline を書き換えません。baseline の既存ファイルは [baseline_manifest.json](baseline_manifest.json) で照合します。正典の個体状態、Persona、continuity、恒久的なメモリ更新からも独立しています。

| ファイル | 責務 |
|---|---|
| [env.py](env.py) | 行動で観測が変化する tensor 環境、情報制約付き教師、snapshot/fork |
| [train.py](train.py) | 系列全体の教師模倣、paired PPO、stage付きcheckpoint |
| [launch.py](launch.py) | 物理GPUごとの独立ジョブキュー |
| [evaluate.py](evaluate.py) | 介入、記憶、OOD、seed単位の統計と結果統合 |
| [audit.py](audit.py) | seed分割、生成episode指紋、baseline照合 |
| [language.py](language.py) | 外部記録のserializer、HTTP、厳密なJSON parser |
| [j72_eval.py](j72_eval.py) | 実応答をCoreへ戻す評価と障害fixture |
| [gui.py](gui.py) | 同期済み成果物を読む日本語Mac GUI |

## 環境と行動

`ActiveInfoEnv(num_envs, device, seed, config)` は PyTorch 上で環境を並列化します。主要条件は 1024 環境、48 transition/episode です。同期episodeを明示的に `reset()` し、`step(actions)` は次観測・報酬・終了mask・評価用infoを返します。

Core の入力は `[N,16]` の float vector です。自然言語、token、scenario ID、seed、wall-clock、latentの正解は入力しません。取得機会を知らせる合成の物理的特徴と、公開されたサービス条件は入力に含みます。

| channel | 内容 | channel | 内容 |
|---:|---|---:|---|
| 0 | motion | 8 | speech_activity |
| 1 | motion_delta | 9 | acquired_value |
| 2 | sound_energy | 10 | acquired_confidence |
| 3 | light_level | 11 | resource_level |
| 4 | proximity | 12 | language_cost / 1.2 |
| 5 | touch | 13 | language_reliability |
| 6 | temperature_delta | 14 | decision_ready |
| 7 | task_relevance | 15 | language_latency / 16 |

取得情報は二値のカテゴリを数値化し、値と信頼度を分けて返します。通常の観測は `[0,1]` に収めます。channel 14 が 1 の最終stepにだけ回答を確定します。この準備flagは A/B 両方で同じです。

| 行動 | 最終判断より前 | 最終判断時 |
|---|---|---|
| `IGNORE` | 観測を改善せず経過 | 無関係な刺激・慣れ課題の回答 |
| `WAIT` | 資源回復、時間経過、待機費用 | 情報課題の正解にはならない |
| `ORIENT` | 2〜4 step の物理センサー改善、費用 | 情報課題の正解にはならない |
| `OBSERVE` | 利用可能な追加センサーを取得、費用 | 二値回答 A |
| `RECALL` | 過去に実際に見たcueを再提示、費用。strict memoryでは無効 | 二値回答 B |
| `INVOKE_LANGUAGE` | 1 episodeにつき1回の応答予約、費用 | 呼び出しだけで成功にはならない |

最終報酬は成功 `+1`、失敗 `-1`。取得費用・言語費用・待機費用を差し引き、慣れ課題には中間報酬を与えます。言語応答待ちの費用は `IGNORE` でも発生します。latency 0 は CALL 直後の次観測へ返却し、latency L はそこから追加 L transition 後です。再CALLは重複請求しません。

### 情報制約と公平性

6つの課題は、言語だけで判別できる A/B、遅延記憶、ORIENT による観測改善、OBSERVE による追加情報、慣れと再反応、無関係な刺激です。

言語課題の A/B は、同じseed・反対のlatentで生成しても、すべての非言語行動について応答前の観測履歴が bit単位で一致します。答えは均等な二値で、言語を使わないBayes基準の期待成功率は50%です。有限評価の観測値には標本変動があります。

strict memory は t=0 のcueを消し、後からの RECALL を無効化します。このsubsetでは資源の**観測値**も固定し、MLPがcueに応じた行動の費用を外部メモリとして利用する経路を塞ぎます。内部の費用計算は継続します。全6行動で分岐した後の観測一致もテストします。nonstrictのRECALLは、ノイズを含む保存済みの実測値と信頼度を再提示します。

教師は過去に取得した情報だけを保持し、未取得の最終回答はlatentから読まず固定のBayes guessにします。教師が記憶するのは実際に見たcueです。取得済みの言語結果が最終判断まで表示されるので、このsubsetでMLPが成功することは想定内です。GRUの記憶優位性はstrict memoryとhidden-state介入で別に測ります。

### 費用・信頼性・遅延

学習時にはepisodeごとに以下を混ぜ、条件をCoreへ告知します。

- 費用：`0, .01, .02, .05, .10, .20, .40, .80, 1.20`
- 信頼性：`1.00, .90, .75, .50`
- 遅延：`0, 1, 2, 4, 8, 16`

解析的な言語取得価値は `2 × (max(q,1−q)−0.5) − cost − wait_cost × latency` です。`+1/-1` 報酬では完全な情報の利益が最大1になるため、指定された初期費用範囲に `.8, 1.2` を追加し、呼ぶ／呼ばないの閾値を横断します。信頼性と応答モデルが既知という条件付きの参照値で、任意の破損応答に対する全知の上限ではありません。

## 有限の学習計画

主要比較は MLP、GRU64、GRU128 の各8 independent seedsです。初期の64-update試行24runは `artifacts/runs/` に保存します。そのvalidationだけを根拠に、全構造へ共通の256-update試行を一度追加し、`artifacts/primary/` に隔離します。初期とprimaryを混ぜてseed数を増やしません。追加の予算探索は行いません。

主要教師模倣は Adam `.001`、系列全体の学習です。前半は教師行動、後半は各stepで80%教師／20%学習policyを混ぜ、訪れた状態で教師ラベルを付けます。行動頻度の重み付けに加えて最終判断を32倍、取得行動を4倍に重み付けします。

checkpoint選択は固定validationの `scenario_macro_success + .1 × episode_reward` に限定します。held-outでseedやcheckpointを選びません。主要比較は `imitation_best.pt` を使い、finalも別に評価します。

### PPOの単独要因比較

GRU128の各seedで同じ `imitation_best.pt` のmodel・optimizerから開始し、rollout RNGを揃えます。各条件は8 updates、2 PPO epochs、256環境のminibatchで、系列を途中分割しません。

| 条件 | B1からの変更 |
|---|---|
| B0 | 教師模倣の比較元。PPOなし |
| B1 | lr `.001`、gamma `.99`、GAE lambda `.95`、ratio clip `.2`、value係数 `.5`、entropy `.01`、advantage標準化、勾配clip `1.0` |
| B2 | 学習報酬の言語費用項だけ0。告知条件・物理資源費用・未来の観測は維持。評価費用は通常条件 |
| B3 | policy/optimizer学習率 `.0001` |
| B4 | critic headだけ4 warm-up updates。Coreとactorを固定し、終了後に共通rollout RNGを復元 |
| B5 | value errorをrollout returnの標準偏差で追加正規化 |
| B6 | 固定した模倣policyへの `.1 × KL(new || imitation)` を追加 |
| B7 | 勾配clipを `.25` に強化 |

B1には既にadvantage標準化と勾配clipがあります。B5/B7は「有無」の比較ではなく、追加のvalue scaling／より強いclipの比較です。B4の追加計算量も記録します。組合せ条件はvalidationで有効性が確認された要因に限り、有限の候補として扱います。

### update・epoch・checkpoint

1 updateは並列episodeの収集とその更新です。PPOのepochは同じrolloutへの最適化反復です。`transitions` は `update × num_envs × episode_length` として記録します。

- `initial.pt`：そのrunを開始するmodel・optimizer・RNG。
- `imitation_best.pt` / `imitation_final.pt`：教師模倣段階の選択済み／最終。
- `ppo_best.pt` / `ppo_final.pt`：PPO段階の選択済み／最終。

checkpointにはstage、update、config、model、optimizer、Python/NumPy/PyTorch/CUDA/environment RNG、model hashを含めます。PPO派生runは親checkpoint hashも記録します。stageの表示と実際の更新段階をテストします。launcherは完了済みrunをskipし、未完了runを保存したまま報告します。汎用的な途中再開CLIは実装していません。

## seedとGPU

独立反復はseedです。episode数を標本数としてp値を膨らませません。seed単位の平均・SD・中央値・bootstrap 95% CI、paired差分・exact sign testを用います。

| 用途 | RNG seed |
|---|---|
| 教師模倣 | `100000 + seed` |
| PPO | `200000 + seed` |
| critic warm-up | `300000 + seed` |
| validation | `700001` |
| held-out | `900001` |

評価はモデル間で同一streamを共有し、学習とは分離します。baseline監査のepisode指紋検査は各stream32episodeの有限検査で、全生成空間の無重複証明ではありません。

RTX 3060とP100はUUIDで固定識別し、1GPUにつき独立した1training processを実行します。偶数seedと奇数seedの物理GPU割当を固定し、paired PPOでも同じGPUを維持します。DDPは使用しません。`launch.py` の `GPU_UUIDS` は対象ホストの実機設定です。別ホストへの再現時はinventoryと照合してください。各子processにはUUIDを `CUDA_VISIBLE_DEVICES` で指定するため、process内の `cuda:0` は物理index 0を意味しません。

## 再現手順

以下はrepository rootから、必要な依存を備えた専用venvで実行します。学習側はPyTorch・NumPy、GUI側はPyQt5・pyqtgraphを使います。実測に使用した厳密な版は成果物の `runtime_packages.txt` と実行環境記録を参照してください。

最初に既存変更とGPU利用状況を確認します。

```bash
git status --short
nvidia-smi --query-gpu=index,uuid,name,memory.free --format=csv
nvidia-smi --query-compute-apps=gpu_uuid,pid --format=csv
python -m unittest experiments.k0_e2_active_info.tests.test_env experiments.k0_e2_active_info.tests.test_train experiments.k0_e2_active_info.tests.test_evaluate experiments.k0_e2_active_info.tests.test_language
```

新しいsmoke保存先を用意し、実機の2GPUで短い一巡を実行します。

```bash
python -m experiments.k0_e2_active_info.launch --artifacts experiments/k0_e2_active_info/artifacts/reproduction-smoke --phase imitation --smoke
```

主要runを新しい名前空間に再現する場合の例です。既存の `primary` は再利用せず保存します。

```bash
python -m experiments.k0_e2_active_info.launch --artifacts experiments/k0_e2_active_info/artifacts/reproduction-primary --phase imitation
python -m experiments.k0_e2_active_info.launch --artifacts experiments/k0_e2_active_info/artifacts/reproduction-primary --phase ppo
python -m experiments.k0_e2_active_info.evaluate --artifacts experiments/k0_e2_active_info/artifacts/reproduction-primary --all --device cpu --threads 4
python -m experiments.k0_e2_active_info.audit --output experiments/k0_e2_active_info/artifacts/reproduction-primary/leakage_audit.json --baseline-root .
```

単独checkpointは `evaluate --checkpoint PATH --artifacts PATH`、保存済み評価の再統合は `evaluate --consolidate-only --artifacts PATH` で実行できます。実験キューは占有中GPUのprocessを終了しません。source hash、runtime、GPU UUID、checkpoint hashと実際の完走件数を結果に添付してください。

## 介入と評価

同じepisodeとpolicyに対して、never/always/oracle/random/matched-rateの言語ゲート、correct/shuffled/random/inverted/delayed/missing等の返却情報、ORIENT/OBSERVE、hidden-stateのreset/noise/quantizationを比較します。matched-rateは受理されたCALL数を照合します。CALL/NO CALLは同じsnapshotとCore stateからforkし、報酬・成功・遅延・下流行動の差を保存します。

記憶遅延は `8,16,24,40,80,160,320,640`。通常評価256episode、長い記憶評価64episodeを既定とし、実際の件数を各結果へ記録します。50%を横切る最初の検査点は離散的な観測であり、未観測区間や無限の記憶保持を推定しません。OOD、慣れ曲線、channel/cue/time/prefix/suffixのshortcut検査も独立に記録します。

## J72を通す閉ループ

学習中はscripted backendだけを利用します。J72代表policyはGRU64/128それぞれvalidationのみで選択します。実際のendpointはコマンド引数で渡し、文書や設定にprivate endpointを埋め込みません。

```bash
python -m experiments.k0_e2_active_info.j72_eval --artifacts experiments/k0_e2_active_info/artifacts/reproduction-primary --endpoint "$J72_ENDPOINT"
```

この評価は、Coreから見えない**合成外部記録のA/Bカテゴリを抽出**する課題です。言語器官にだけ記録を渡し、返却JSONの `category`, `confidence`, `evidence_id` を厳密に検査します。不正な値の修復・丸めは行いません。parserを通った数値だけを `inject_language_response()` で戻し、次のCore入力と最終行動まで追跡します。`language_backend='external'` は、未応答や失敗時にscriptedの正解へfallbackしません。

API成功、parse成功、意味的正解、Coreの最終成功、実HTTP遅延は別々に記録します。HTTPの壁時計秒と環境内の固定遅延stepは別の量です。timeout/503/malformed/empty/contradiction/slowの障害評価には制御されたtransport fixtureを使い、実J72の測定と区別します。学習ゲートが一度も開かなければその失敗を保存し、強制gateの診断結果を別枠にします。

このbinary記録抽出から、自由な言語理解・一般的な推論・現実の情報源選択能力を結論しません。J72サービスの起動停止はこの評価CLIの責務に含めず、一時起動した場合は終了後に元の状態へ戻します。

## 日本語Mac GUI

GUIは、Macへ同期済みのartifactsフォルダーを読み取る専用ビューアーです。学習処理や操作APIは持たず、終了してもリモート学習には影響しません。既存K0-EのGUIとは別windowで動作します。

```bash
python -m experiments.k0_e2_active_info.gui --artifacts experiments/k0_e2_active_info/artifacts/primary
QT_QPA_PLATFORM=offscreen python -m unittest experiments.k0_e2_active_info.tests.test_gui
```

5秒ごとの再読込で、run状態、学習曲線、scenario成績、10評価グラフ、`j72_results.json` の行動・取得情報・16channel・hidden norm・言語状態・費用・資源を表示します。フォルダーはGUIから切り替え可能です。最新記録時刻とMacでの読込時刻を分け、同期の遅れを学習停止と混同しないようにします。latentはデバッグcheckboxを明示的に有効にしたときだけ表示します。

## 完了と研究上の判定

実装の検証は、行動による観測変化、pre-call ambiguity、response注入、費用と遅延、RNG復元、stage整合、GUIの読み取りを対象とします。

研究仮説の判定では、取得情報への下流行動の依存、never-call／matched-rate randomからの改善、strict memoryのGRU優位性、費用・信頼性・遅延への適応、PPO退行の原因、OODと実J72の成功を別々に評価します。テストが通っても、これらの仮説は棄却・未達になり得ます。

`run_summary.json/csv`、介入・retention・OOD・VoI・PPOの各JSON、グラフ、実行証跡、source manifest、checkpoint hashと最終REPORTを照合して完了判定します。failed run・未検証層・未達の成功条件も保存し、K1への移行判断はその結果に基づいて行います。
