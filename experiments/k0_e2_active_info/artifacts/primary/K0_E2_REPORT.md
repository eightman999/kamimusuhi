# K0-E2 Active Information Acquisition 実験報告

実行完了：**完了**。研究成功条件：**FAIL**。

## 1. 結論

GRU64：returned informationの利用（correct−shuffled、言語subset）0.4889 ± 0 / 中央値 0.4889 / CI [0.4889, 0.4889] (n=8); exact sign p=0.007812; PASS。

GRU128：returned informationの利用（correct−shuffled、言語subset）0.4889 ± 0 / 中央値 0.4889 / CI [0.4889, 0.4889] (n=8); exact sign p=0.007812; PASS。

GRU128の訓練時刻での記憶依存（delay=47、normal−reset every step）0.5469 ± 0 / 中央値 0.5469 / CI [0.5469, 0.5469] (n=8); exact sign p=0.007812; PASS。未知時刻への記憶転移：FAIL。

現行PPO B1−B0成功率差：-0.3257 ± 0.2214 / 中央値 -0.3223 / CI [-0.4736, -0.1792] (n=8); exact sign p=0.01562; FAIL。負値は退行であり改善扱いしない。

cost 0→1.2のcall率低下：GRU64 FAIL（平均低下 0）、GRU128 PARTIAL（平均低下 0.0009766）。高い成功率と合理的な取得cost判断は別の課題である。

J72の実API・parse・意味情報・Core閉ループ判定：FAIL。HTTP成功だけで言語器官の成功としない。

validation seed700001の選択score（scenario macro成功率 + 0.1×episode reward）の8 seed平均：MLP 0.799198、GRU64 1.077956、GRU128 1.101479。best Coreは GRU128。軽量候補GRU64は別に保持し、test最高seedで選び直さない。

## 2. 研究質問と仮説

H1：現在の観測だけで解けない記憶。H2：ORIENT/OBSERVEによる能動観測。H3：言語器官を情報取得に利用。H4：cost・reliability・latencyへの条件依存。H5：戻った情報が後続行動と成功を変える。H6：PPO退行の因子切り分け。相関、call率、HTTP 200だけを成功根拠にしない。

## 3. K0-Eとの差分

K0-Eのcheckpoint・集約成果物をimmutable baselineとして保全した独立namespaceである。K0-E2では行動が未来観測を変え、CALL→応答→固定長数値interface→Core→次行動を閉じる。K0は常時稼働する非言語controller、J72は条件付きで参照する言語器官と位置付ける。Git push・外部公開は行わない。baseline hashの確認結果は後述する。

## 4. Environment設計

16次元float観測、6行動、同期した48-step部分観測環境。言語A/B pairは非言語行動だけでは履歴が一致し、最終正解はOBSERVE/RECALLで異なる。正解latentとscenario IDはpolicy入力にしない。一方、合成的な物理affordanceから必要な取得操作の種類を識別できるため、未知task type推論の証明ではない。deadline flagとservice条件は公開する。strict-memoryではcue消失後の外部RECALL再提示とcue依存の環境書込みを抑制する。

## 5. 情報取得設計

ORIENTは短期SNR改善、OBSERVEは物理的追加情報、RECALLは過去に実際に観測した情報の再提示を意味する。strict-memoryでは消えたcueを再取得できない。言語callは1 episodeに1回受理しcostを課し、遅延後のcategory/confidence情報を観測へ戻す。callした瞬間には成功しない。language latency=0はcallの直後の次観測が最速で、追加L stepなら到着までL+1遷移。言語応答はdecisionまで持続表示されるため、CALL後にMLPが解けても漏洩とは限らない。

## 6. 訓練条件とprotocol変更

| 項目 | 保存config値 |
|---|---|
| imitation updates | 256 |
| 並列環境 | 1024 |
| PPO updates | 8 |

primaryは3構成×8独立seed、256 imitation更新（前半128 teacher、後半128はteacher 80%/learner 20% DAgger exposure）。64更新pilotは別保存し、held-outを見る前のvalidation学習不足を根拠に共通のfresh protocolを1度だけ追加した。teacher-only期間とDAgger期間も変わるので、64→256差を純粋な計算予算の因果効果として解釈しない。1024並列環境、48 step、全sequence勾配、learning rate .001、決定step重み32・取得行動重み4を用いる。validation seed=700001、held-out=900001。評価時の基本条件はcost=.05、reliability=1、追加latency=2。

## 7. Architecture比較

| 指標：mean±SD / median / 95% CI | MLP | GRU64 | GRU128 |
|---|---|---|---|
| 課題成功率 | 0.7661 ± 0.008465 / 中央値 0.7676 / CI [0.7607, 0.7715] (n=8) | 0.9829 ± 0.04678 / 中央値 1 / CI [0.9497, 1] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| シナリオmacro成功率 | 0.7366 ± 0.0084 / 中央値 0.7381 / CI [0.7313, 0.7419] (n=8) | 0.9808 ± 0.05252 / 中央値 1 / CI [0.9435, 1] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| 1エピソード累積報酬 | 0.4978 ± 0.01679 / 中央値 0.5007 / CI [0.4871, 0.5084] (n=8) | 0.9635 ± 0.1 / 中央値 0.9988 / CI [0.8923, 1.001] (n=8) | 1.004 ± 0.001174 / 中央値 1.004 / CI [1.003, 1.005] (n=8) |
| oracle参照との差（regret） | 0.5217 ± 0.01679 / 中央値 0.5189 / CI [0.5111, 0.5324] (n=8) | 0.05605 ± 0.1 / 中央値 0.02074 / CI [0.01897, 0.1272] (n=8) | 0.01558 ± 0.001174 / 中央値 0.01525 / CI [0.01496, 0.01643] (n=8) |
| 受理された言語callを含むepisode割合 | 0.1758 ± 0 / 中央値 0.1758 / CI [0.1758, 0.1758] (n=8) | 0.3672 ± 0 / 中央値 0.3672 / CI [0.3672, 0.3672] (n=8) | 0.3677 ± 0.001381 / 中央値 0.3672 / CI [0.3672, 0.3687] (n=8) |
| 必要call再現率 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| 必要call適合率 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.4787 ± 0 / 中央値 0.4787 / CI [0.4787, 0.4787] (n=8) | 0.4781 ± 0.001782 / 中央値 0.4787 / CI [0.4768, 0.4787] (n=8) |
| 必要call F1 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.6475 ± 0 / 中央値 0.6475 / CI [0.6475, 0.6475] (n=8) | 0.6469 ± 0.001635 / 中央値 0.6475 / CI [0.6457, 0.6475] (n=8) |
| 不要episodeでのcall割合 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) | 0.2322 ± 0 / 中央値 0.2322 / CI [0.2322, 0.2322] (n=8) | 0.2328 ± 0.001676 / 中央値 0.2322 / CI [0.2322, 0.234] (n=8) |
| 必要episodeでの見逃し率 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) |
| 行動macro-F1 | 0.5679 ± 0.002601 / 中央値 0.5683 / CI [0.5662, 0.5695] (n=8) | 0.7061 ± 0.04101 / 中央値 0.7156 / CI [0.6787, 0.731] (n=8) | 0.8727 ± 0.01113 / 中央値 0.873 / CI [0.8658, 0.8801] (n=8) |
| 厳密記憶成功率 | 0.4865 ± 0 / 中央値 0.4865 / CI [0.4865, 0.4865] (n=8) | 0.9966 ± 0.009555 / 中央値 1 / CI [0.9899, 1] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| 慣れ系列成功率 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) | 0.8882 ± 0.3163 / 中央値 1 / CI [0.6645, 1] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| 新奇刺激反応率 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| parameter数 | 1.959e+04 ± 0 / 中央値 1.959e+04 / CI [1.959e+04, 1.959e+04] (n=8) | 1.62e+04 ± 0 / 中央値 1.62e+04 / CI [1.62e+04, 1.62e+04] (n=8) | 5.697e+04 ± 0 / 中央値 5.697e+04 / CI [5.697e+04, 5.697e+04] (n=8) |
| CPU推論中央値ms | 0.01873 ± 0.0002207 / 中央値 0.01871 / CI [0.01859, 0.01888] (n=8) | 0.02877 ± 0.0007993 / 中央値 0.02916 / CI [0.02822, 0.02925] (n=8) | 0.03103 ± 0.00219 / 中央値 0.03006 / CI [0.02995, 0.0326] (n=8) |
| CPU推論p95 ms | 0.01911 ± 0.0002119 / 中央値 0.0191 / CI [0.01897, 0.01925] (n=8) | 0.03034 ± 0.0009206 / 中央値 0.03057 / CI [0.02974, 0.03089] (n=8) | 0.03277 ± 0.002493 / 中央値 0.03189 / CI [0.03155, 0.03457] (n=8) |
| 学習transitions/秒 | 5.622e+05 ± 1.022e+04 / 中央値 5.628e+05 / CI [5.555e+05, 5.684e+05] (n=8) | 4.85e+05 ± 2672 / 中央値 4.861e+05 / CI [4.832e+05, 4.866e+05] (n=8) | 3.246e+05 ± 2.677e+04 / 中央値 3.25e+05 / CI [3.065e+05, 3.43e+05] (n=8) |

| 構成 | シナリオ | 成功率 |
|---|---|---|
| MLP | 言語曖昧性 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| MLP | 厳密な記憶 | 0.4865 ± 0 / 中央値 0.4865 / CI [0.4865, 0.4865] (n=8) |
| MLP | 能動ORIENT | 0.9331 ± 0.0504 / 中央値 0.9419 / CI [0.9012, 0.9651] (n=8) |
| MLP | 能動OBSERVE | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| MLP | 慣れ | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) |
| MLP | 無関係刺激 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| GRU64 | 言語曖昧性 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| GRU64 | 厳密な記憶 | 0.9966 ± 0.009555 / 中央値 1 / CI [0.9899, 1] (n=8) |
| GRU64 | 能動ORIENT | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| GRU64 | 能動OBSERVE | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| GRU64 | 慣れ | 0.8882 ± 0.3163 / 中央値 1 / CI [0.6645, 1] (n=8) |
| GRU64 | 無関係刺激 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| GRU128 | 言語曖昧性 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| GRU128 | 厳密な記憶 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| GRU128 | 能動ORIENT | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| GRU128 | 能動OBSERVE | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| GRU128 | 慣れ | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |
| GRU128 | 無関係刺激 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) |

| 構成 | 行動 | precision | recall | F1 |
|---|---|---|---|---|
| MLP | IGNORE | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.9474 ± 0.0003189 / 中央値 0.9473 / CI [0.9472, 0.9476] (n=8) | 0.973 ± 0.0001682 / 中央値 0.9729 / CI [0.9729, 0.9731] (n=8) |
| MLP | WAIT | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) |
| MLP | ORIENT | 0.1693 ± 0.0009127 / 中央値 0.169 / CI [0.1688, 0.17] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.2896 ± 0.001334 / 中央値 0.2892 / CI [0.2888, 0.2906] (n=8) |
| MLP | OBSERVE | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.8304 ± 0.0168 / 中央値 0.8333 / CI [0.8198, 0.8411] (n=8) | 0.9073 ± 0.01007 / 中央値 0.9091 / CI [0.9009, 0.9137] (n=8) |
| MLP | RECALL | 0.584 ± 0.008744 / 中央値 0.5854 / CI [0.5784, 0.5895] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.7373 ± 0.006985 / 中央値 0.7385 / CI [0.7329, 0.7417] (n=8) |
| MLP | INVOKE_LANGUAGE | 0.3333 ± 0 / 中央値 0.3333 / CI [0.3333, 0.3333] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.5 ± 0 / 中央値 0.5 / CI [0.5, 0.5] (n=8) |
| GRU64 | IGNORE | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.9694 ± 0.008958 / 中央値 0.9712 / CI [0.9633, 0.9748] (n=8) | 0.9844 ± 0.004628 / 中央値 0.9854 / CI [0.9813, 0.9872] (n=8) |
| GRU64 | WAIT | 0.5498 ± 0.2402 / 中央値 0.4973 / CI [0.4025, 0.7169] (n=8) | 0.5 ± 0 / 中央値 0.5 / CI [0.5, 0.5] (n=8) | 0.5028 ± 0.1073 / 中央値 0.4986 / CI [0.433, 0.571] (n=8) |
| GRU64 | ORIENT | 0.449 ± 0.04409 / 中央値 0.4505 / CI [0.4214, 0.4767] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.6186 ± 0.04241 / 中央値 0.621 / CI [0.5916, 0.6449] (n=8) |
| GRU64 | OBSERVE | 0.8216 ± 0.1128 / 中央値 0.7522 / CI [0.7573, 0.8973] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.8985 ± 0.06486 / 中央値 0.8586 / CI [0.8615, 0.9425] (n=8) |
| GRU64 | RECALL | 0.7136 ± 0.2614 / 中央値 0.6577 / CI [0.5472, 0.8763] (n=8) | 0.9985 ± 0.004209 / 中央値 1 / CI [0.9955, 1] (n=8) | 0.8077 ± 0.1875 / 中央値 0.7931 / CI [0.685, 0.9235] (n=8) |
| GRU64 | INVOKE_LANGUAGE | 0.271 ± 0.04367 / 中央値 0.2394 / CI [0.2499, 0.3026] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.4248 ± 0.05324 / 中央値 0.3863 / CI [0.3991, 0.4634] (n=8) |
| GRU128 | IGNORE | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.9846 ± 0.001523 / 中央値 0.9848 / CI [0.9837, 0.9857] (n=8) | 0.9923 ± 0.0007731 / 中央値 0.9923 / CI [0.9918, 0.9928] (n=8) |
| GRU128 | WAIT | 0.9157 ± 0.08841 / 中央値 0.9391 / CI [0.8576, 0.9696] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.954 ± 0.04911 / 中央値 0.9682 / CI [0.9212, 0.9841] (n=8) |
| GRU128 | ORIENT | 0.5117 ± 0.01572 / 中央値 0.5064 / CI [0.504, 0.523] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.6768 ± 0.01352 / 中央値 0.6723 / CI [0.6702, 0.6866] (n=8) |
| GRU128 | OBSERVE | 0.9412 ± 0.08581 / 中央値 0.9699 / CI [0.8792, 0.9843] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.9678 ± 0.04936 / 中央値 0.9847 / CI [0.9321, 0.992] (n=8) |
| GRU128 | RECALL | 0.9971 ± 0.008222 / 中央値 1 / CI [0.9913, 1] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.9985 ± 0.004159 / 中央値 1 / CI [0.9956, 1] (n=8) |
| GRU128 | INVOKE_LANGUAGE | 0.4781 ± 0.001782 / 中央値 0.4787 / CI [0.4768, 0.4787] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.6469 ± 0.001635 / 中央値 0.6475 / CI [0.6457, 0.6475] (n=8) |

混同行列は正解行×予測列、行動順 IGNORE/WAIT/ORIENT/OBSERVE/RECALL/INVOKE_LANGUAGE で raw run_summary.json に保存。集計はseedごとの各指標を等重みとする。

全scalar指標の再計算値は [report_statistics.json](report_statistics.json)。meanは独立seedを等重みとし、episodeを独立反復にしない。oracle regretは情報制約付きanalytic参照reward−learned rewardであり、参照policyの大域最適性は証明していない。

## 8. Memory結果

| 構成 | 訓練delay47 normal | normal−reset every step | GRU normal−MLP normal |
|---|---|---|---|
| MLP | 0.5449 ± 0.005524 / 中央値 0.5469 / CI [0.541, 0.5469] (n=8) | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8); exact sign p=1; FAIL | memoryless control |
| GRU64 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.8203 ± 0.227 / 中央値 0.9609 / CI [0.6602, 0.9434] (n=8); exact sign p=0.007812; PASS | 0.4551 ± 0.005524 / 中央値 0.4531 / CI [0.4531, 0.459] (n=8); exact sign p=0.007812; PASS |
| GRU128 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.5469 ± 0 / 中央値 0.5469 / CI [0.5469, 0.5469] (n=8); exact sign p=0.007812; PASS | 0.4551 ± 0.005524 / 中央値 0.4531 / CI [0.4531, 0.459] (n=8); exact sign p=0.007812; PASS |

この追加診断は既知の訓練時刻47のcontrol不足を補うもので、好成績のtest delayを探索して選んだものではない。primary評価とcheckpointを保全し、学習・model選択には使わない。cue A/Bが消えた後の観測bit一致とnormal policyの最終行動分岐を同時に確認する。 [trained_delay_ablation.json](trained_delay_ablation.json)

| 構成 | cue消失後bit一致seed | balanced A/B成功率 | 最終行動変化率 | GRU−MLP paired成功率 |
|---|---|---|---|---|
| MLP | 8/8 | 0.5 ± 0 / 中央値 0.5 / CI [0.5, 0.5] (n=8) | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) | memoryless control |
| GRU64 | 8/8 | 0.999 ± 0.002762 / 中央値 1 / CI [0.9971, 1] (n=8) | 0.998 ± 0.005524 / 中央値 1 / CI [0.9941, 1] (n=8) | 0.499 ± 0.002762 / 中央値 0.5 / CI [0.4971, 0.5] (n=8); exact sign p=0.007812; PASS |
| GRU128 | 8/8 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 0.5 ± 0 / 中央値 0.5 / CI [0.5, 0.5] (n=8); exact sign p=0.007812; PASS |

| 構成 | 未知のdelay | normal記憶成功率 |
|---|---|---|
| MLP | 8 | 0.5469 ± 0 / 中央値 0.5469 / CI [0.5469, 0.5469] (n=8) |
| MLP | 16 | 0.5469 ± 0 / 中央値 0.5469 / CI [0.5469, 0.5469] (n=8) |
| MLP | 24 | 0.5449 ± 0.005524 / 中央値 0.5469 / CI [0.541, 0.5469] (n=8) |
| MLP | 40 | 0.5488 ± 0.005524 / 中央値 0.5469 / CI [0.5469, 0.5527] (n=8) |
| MLP | 80 | 0.5469 ± 0 / 中央値 0.5469 / CI [0.5469, 0.5469] (n=8) |
| MLP | 160 | 0.543 ± 0.01105 / 中央値 0.5469 / CI [0.5352, 0.5469] (n=8) |
| MLP | 320 | 0.5449 ± 0.005524 / 中央値 0.5469 / CI [0.541, 0.5469] (n=8) |
| MLP | 640 | 0.5469 ± 0 / 中央値 0.5469 / CI [0.5469, 0.5469] (n=8) |
| GRU64 | 8 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) |
| GRU64 | 16 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) |
| GRU64 | 24 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) |
| GRU64 | 40 | 0.1367 ± 0.2101 / 中央値 0 / CI [0.02539, 0.291] (n=8) |
| GRU64 | 80 | 0.5801 ± 0.1749 / 中央値 0.5469 / CI [0.5, 0.7051] (n=8) |
| GRU64 | 160 | 0.5801 ± 0.1749 / 中央値 0.5469 / CI [0.5, 0.7051] (n=8) |
| GRU64 | 320 | 0.5801 ± 0.1749 / 中央値 0.5469 / CI [0.5, 0.7051] (n=8) |
| GRU64 | 640 | 0.5801 ± 0.1749 / 中央値 0.5469 / CI [0.5, 0.7051] (n=8) |
| GRU128 | 8 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) |
| GRU128 | 16 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) |
| GRU128 | 24 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) |
| GRU128 | 40 | 0.3477 ± 0.4423 / 中央値 0.1328 / CI [0.06641, 0.6309] (n=8) |
| GRU128 | 80 | 0.7637 ± 0.2615 / 中央値 0.8359 / CI [0.5956, 0.9316] (n=8) |
| GRU128 | 160 | 0.7383 ± 0.2815 / 中央値 0.7734 / CI [0.5449, 0.9316] (n=8) |
| GRU128 | 320 | 0.6699 ± 0.2752 / 中央値 0.5 / CI [0.5215, 0.8633] (n=8) |
| GRU128 | 640 | 0.6699 ± 0.2752 / 中央値 0.5 / CI [0.5215, 0.8633] (n=8) |

事前指定8/16/24/40/80/160/320/640 stepではnormal/reset each step/reset8/noise/quantizeを比較する。訓練は47であり、これらは記憶長だけでなく決定時刻OODを含む。未知時刻での非単調な破綻は時刻generalizationの失敗として残し、単一memory horizonや50% crossingは定義しない。各条件64 episodeが標準だが、実数は各JSONのepisodesに従う。 [retention_curve.png](retention_curve.png)

## 9. Active sensingと慣れ

| 構成 | 取得操作 | learned−disabled成功率 |
|---|---|---|
| MLP | ORIENT | 0.4272 ± 0.03729 / 中央値 0.4297 / CI [0.4019, 0.4497] (n=8); exact sign p=0.007812; PASS |
| MLP | OBSERVE | 0.5273 ± 0 / 中央値 0.5273 / CI [0.5273, 0.5273] (n=8); exact sign p=0.007812; PASS |
| GRU64 | ORIENT | 0.7637 ± 0.2526 / 中央値 0.7637 / CI [0.5864, 0.9409] (n=8); exact sign p=0.007812; PASS |
| GRU64 | OBSERVE | 0.5273 ± 0 / 中央値 0.5273 / CI [0.5273, 0.5273] (n=8); exact sign p=0.007812; PASS |
| GRU128 | ORIENT | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8); exact sign p=0.007812; PASS |
| GRU128 | OBSERVE | 0.6455 ± 0.2188 / 中央値 0.5273 / CI [0.5273, 0.7637] (n=8); exact sign p=0.007812; PASS |

forced/disabled/oracle/random/learnedを同一seedで評価。強制は開始時に1回の取得操作を与える介入で、あらゆる時点に強制する操作ではない。habituationはfirst ORIENT→repeat IGNORE→novel ORIENTを要求し、novelty強度・反復回数・gap・noiseの有限sweepを保存する。シナリオのaffordanceと既定のevent構造への依存は残る。 [active_sensing.json](active_sensing.json) / [habituation_results.json](habituation_results.json)

## 10. Language VoI

| 構成 | 比較（call率差） | seed対応結果 |
|---|---|---|
| MLP | cost: 0−1.2 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8); exact sign p=1; FAIL |
| MLP | reliability: 1.0−0.5 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8); exact sign p=1; FAIL |
| MLP | latency: 0−16 | 0.05859 ± 0.0781 / 中央値 0.01562 / CI [0.01074, 0.1104] (n=8); exact sign p=0.125; PARTIAL |
| GRU64 | cost: 0−1.2 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8); exact sign p=1; FAIL |
| GRU64 | reliability: 1.0−0.5 | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8); exact sign p=1; FAIL |
| GRU64 | latency: 0−16 | -0.021 ± 0.03456 / 中央値 0 / CI [-0.04542, -0.002441] (n=8); exact sign p=0.25; PARTIAL |
| GRU128 | cost: 0−1.2 | 0.0009766 ± 0.001808 / 中央値 0 / CI [0, 0.002441] (n=8); exact sign p=0.5; PARTIAL |
| GRU128 | reliability: 1.0−0.5 | 0.0004883 ± 0.001381 / 中央値 0 / CI [0, 0.001465] (n=8); exact sign p=1; PARTIAL |
| GRU128 | latency: 0−16 | -0.0293 ± 0.05618 / 中央値 0 / CI [-0.06982, 0] (n=8); exact sign p=0.5; PARTIAL |

cost基本grid=0/.01/.02/.05/.10/.20/.40と、±1報酬のanalytic閾値を横断する事前補足=.8/1.2を分けて扱う。後者をtest結果を見た追加tuningとは扱わない。reliability=1/.9/.75/.5、追加latency=0/1/2/4/8/16の一因子sweepである。上表で正差ならcost上昇・信頼度低下・latency増加に伴うcall減少。ただし成功率やrewardを保つかは別指標であり、call減少のみを合理的VoIとしない。 [voi_cost_curve.png](voi_cost_curve.png) / [voi_performance_curve.png](voi_performance_curve.png) / [reliability_curve.png](reliability_curve.png) / [latency_curve.png](latency_curve.png)

## 11. Counterfactualとreturned information

| 構成 | CALL−NO CALL reward | 成功率差 | WAIT step差 | 後続行動の不一致率 |
|---|---|---|---|---|
| MLP | 1.093 ± 0 / 中央値 1.093 / CI [1.093, 1.093] (n=8) | 0.5273 ± 0 / 中央値 0.5273 / CI [0.5273, 0.5273] (n=8) | -44 ± 0 / 中央値 -44 / CI [-44, -44] (n=8) | 0.9462 ± 0 / 中央値 0.9462 / CI [0.9462, 0.9462] (n=8) |
| GRU64 | 1.011 ± 0.004712 / 中央値 1.011 / CI [1.007, 1.013] (n=8) | 0.5273 ± 0 / 中央値 0.5273 / CI [0.5273, 0.5273] (n=8) | -5.376 ± 1.62 / 中央値 -5.686 / CI [-6.341, -4.273] (n=8) | 0.1191 ± 0.04215 / 中央値 0.1164 / CI [0.09224, 0.1458] (n=8) |
| GRU128 | 1.017 ± 0.003946 / 中央値 1.016 / CI [1.015, 1.02] (n=8) | 0.5273 ± 0 / 中央値 0.5273 / CI [0.5273, 0.5273] (n=8) | -6.057 ± 1.973 / 中央値 -5.629 / CI [-7.444, -4.94] (n=8) | 0.1214 ± 0.0429 / 中央値 0.1121 / CI [0.09712, 0.1515] (n=8) |

step1で同一env・model stateからforkし、最初のCALL有無を変え、その後のcallは両群で無効化する。これは単回強制callのcausal estimandで、自然に学習されたgateの平均処置効果ではない。エピソードdeadlineは固定なので総episode長差は0。WAIT回数差、サービス情報到着遅延、HTTP秒を混同しない。

| 構成 | correct−shuffled：言語subset | learned−never：言語subset | learned−matched random：言語subset |
|---|---|---|---|
| MLP | 0.4889 ± 0 / 中央値 0.4889 / CI [0.4889, 0.4889] (n=8); exact sign p=0.007812; PASS | 0.5333 ± 0 / 中央値 0.5333 / CI [0.5333, 0.5333] (n=8); exact sign p=0.007812; PASS | 0.4 ± 0 / 中央値 0.4 / CI [0.4, 0.4] (n=8); exact sign p=0.007812; PASS |
| GRU64 | 0.4889 ± 0 / 中央値 0.4889 / CI [0.4889, 0.4889] (n=8); exact sign p=0.007812; PASS | 0.5333 ± 0 / 中央値 0.5333 / CI [0.5333, 0.5333] (n=8); exact sign p=0.007812; PASS | 0.4667 ± 0.02376 / 中央値 0.4667 / CI [0.4528, 0.4833] (n=8); exact sign p=0.007812; PASS |
| GRU128 | 0.4889 ± 0 / 中央値 0.4889 / CI [0.4889, 0.4889] (n=8); exact sign p=0.007812; PASS | 0.5333 ± 0 / 中央値 0.5333 / CI [0.5333, 0.5333] (n=8); exact sign p=0.007812; PASS | 0.4611 ± 0.0197 / 中央値 0.4556 / CI [0.45, 0.475] (n=8); exact sign p=0.007812; PASS |

matched randomはglobal episode割当と時点を再配置し、受理call数を正確に一致させる。タイミング単独の効果ではない。応答shuffled等の介入は入力情報利用の証拠であり、時系列全体shuffleのoffline診断とは区別する。 [policy_intervention.png](policy_intervention.png) / [response_ablation.png](response_ablation.png)

## 12. PPO ablation

| arm | PPO-final成功率 | arm−B0 imitation-best | arm−B1 |
|---|---|---|---|
| B1 | 0.6743 ± 0.2214 / 中央値 0.6777 / CI [0.5264, 0.8208] (n=8) | -0.3257 ± 0.2214 / 中央値 -0.3223 / CI [-0.4736, -0.1792] (n=8); exact sign p=0.01562; FAIL | 基準 |
| B2 | 0.8135 ± 0.1553 / 中央値 0.8438 / CI [0.7124, 0.9106] (n=8) | -0.1865 ± 0.1553 / 中央値 -0.1562 / CI [-0.2876, -0.08936] (n=8); exact sign p=0.03125; FAIL | 0.1392 ± 0.2623 / 中央値 0.1328 / CI [-0.02832, 0.3057] (n=8); exact sign p=1; PARTIAL |
| B3 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 観測上は変化なし・性能保持（優越性なし、一般的非劣性の証明ではない）; 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) | 0.3257 ± 0.2214 / 中央値 0.3223 / CI [0.1792, 0.4736] (n=8); exact sign p=0.01562; PASS |
| B4 | 0.8027 ± 0.2242 / 中央値 0.8789 / CI [0.6431, 0.9287] (n=8) | -0.1973 ± 0.2242 / 中央値 -0.1211 / CI [-0.3569, -0.07125] (n=8); exact sign p=0.03125; FAIL | 0.1284 ± 0.2265 / 中央値 0.2012 / CI [-0.01904, 0.2715] (n=8); exact sign p=0.7266; PARTIAL |
| B5 | 0.4307 ± 0.1025 / 中央値 0.4219 / CI [0.3716, 0.5039] (n=8) | -0.5693 ± 0.1025 / 中央値 -0.5781 / CI [-0.6284, -0.4961] (n=8); exact sign p=0.007812; FAIL | -0.2437 ± 0.2237 / 中央値 -0.209 / CI [-0.396, -0.1074] (n=8); exact sign p=0.01562; FAIL |
| B6 | 0.96 ± 0.1132 / 中央値 1 / CI [0.8799, 1] (n=8) | -0.04004 ± 0.1132 / 中央値 0 / CI [-0.1201, 0] (n=8); exact sign p=1; PARTIAL | 0.2856 ± 0.215 / 中央値 0.2461 / CI [0.1548, 0.4336] (n=8); exact sign p=0.01562; PASS |
| B7 | 0.9565 ± 0.06885 / 中央値 1 / CI [0.9062, 0.9946] (n=8) | -0.04346 ± 0.06885 / 中央値 0 / CI [-0.09375, -0.005371] (n=8); exact sign p=0.25; PARTIAL | 0.2822 ± 0.2259 / 中央値 0.3223 / CI [0.1377, 0.4326] (n=8); exact sign p=0.03125; PASS |
| B8 | 1 ± 0 / 中央値 1 / CI [1, 1] (n=8) | 観測上は変化なし・性能保持（優越性なし、一般的非劣性の証明ではない）; 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) | 0.3257 ± 0.2214 / 中央値 0.3223 / CI [0.1792, 0.4736] (n=8); exact sign p=0.01562; PASS |

B1現行PPO、B2言語cost reward項のみゼロ、B3低LR .0001、B4 critic headのみ4更新warm-up、B5 return SDによるvalue loss scale、B6 imitation KL anchor .1、B7強いgradient clip .25。B1は既にadvantage normalizationとclip1.0を持つため、B5/B7を機能の有無比較とは呼ばない。各seedは同じimitation-best parent/optimizerと共通rollout RNGから始める。B4の追加computeは他因子と別記する。単一変更以外の交絡をparent hash/RNG証跡で確認する。複合候補B8等が存在すれば表へ自動追加するが、validationのみで選んだことの証拠が必要。改善がなくてもimitationを保持し、PPO成功としない。 [ppo_ablation.png](ppo_ablation.png)

複合候補のvalidation限定選択（testは不使用）：B3, B6。

| 因子 | final validation score | B1との差 | exact sign p | 採用候補 |
|---|---|---|---|---|
| B1 | 0.7083 ± 0.2553 / 中央値 0.7135 / CI [0.5379, 0.8785] (n=8) | 0 ± 0 / 中央値 0 / CI [0, 0] (n=8) | 1 | いいえ |
| B2 | 0.8729 ± 0.1886 / 中央値 0.8996 / CI [0.7515, 0.9912] (n=8) | 0.1646 ± 0.3057 / 中央値 0.1538 / CI [-0.02888, 0.3642] (n=8) | 0.7266 | いいえ |
| B3 | 1.102 ± 0.000143 / 中央値 1.102 / CI [1.101, 1.102] (n=8) | 0.3932 ± 0.2552 / 中央値 0.3881 / CI [0.2231, 0.5635] (n=8) | 0.007812 | はい |
| B4 | 0.8578 ± 0.2657 / 中央値 0.9407 / CI [0.6723, 1.009] (n=8) | 0.1494 ± 0.2648 / 中央値 0.225 / CI [-0.02044, 0.3174] (n=8) | 0.2891 | いいえ |
| B5 | 0.4218 ± 0.127 / 中央値 0.4114 / CI [0.3488, 0.5139] (n=8) | -0.2865 ± 0.2664 / 中央値 -0.2562 / CI [-0.468, -0.1264] (n=8) | 0.07031 | いいえ |
| B6 | 1.056 ± 0.1296 / 中央値 1.102 / CI [0.9642, 1.102] (n=8) | 0.3475 ± 0.2524 / 中央値 0.2946 / CI [0.1976, 0.5205] (n=8) | 0.007812 | はい |
| B7 | 1.05 ± 0.0815 / 中央値 1.1 / CI [0.9934, 1.095] (n=8) | 0.3421 ± 0.2672 / 中央値 0.3879 / CI [0.1705, 0.5199] (n=8) | 0.2891 | いいえ |

## 13. OOD

| 構成 | named perturbation | 成功率 |
|---|---|---|
| MLP | constant_sensor | 0.5103 ± 0.00978 / 中央値 0.5098 / CI [0.5039, 0.5166] (n=8) |
| MLP | correlated_noise | 0.7427 ± 0.01225 / 中央値 0.75 / CI [0.7344, 0.7495] (n=8) |
| MLP | delayed_sensor | 0.7539 ± 0.009568 / 中央値 0.7578 / CI [0.7476, 0.7598] (n=8) |
| MLP | dropout | 0.4731 ± 0.01053 / 中央値 0.4785 / CI [0.4658, 0.4795] (n=8) |
| MLP | long_delay | 0.7397 ± 0.02548 / 中央値 0.7402 / CI [0.7231, 0.7559] (n=8) |
| MLP | noise_increase | 0.4688 ± 0.01124 / 中央値 0.4648 / CI [0.4619, 0.4761] (n=8) |
| MLP | novel_scale | 0.6353 ± 0.05033 / 中央値 0.6465 / CI [0.6011, 0.6655] (n=8) |
| MLP | partial_inversion | 0.332 ± 0 / 中央値 0.332 / CI [0.332, 0.332] (n=8) |
| MLP | resource_reduction | 0.7017 ± 0.01991 / 中央値 0.6992 / CI [0.688, 0.7139] (n=8) |
| MLP | sensor_failure | 0.5103 ± 0.00978 / 中央値 0.5098 / CI [0.5039, 0.5166] (n=8) |
| MLP | sensor_inversion | 0.3188 ± 0.03327 / 中央値 0.3301 / CI [0.2969, 0.3394] (n=8) |
| MLP | sensor_permutation | 0.2505 ± 0.01171 / 中央値 0.2539 / CI [0.2417, 0.2559] (n=8) |
| MLP | stuck_at | 0.5103 ± 0.00978 / 中央値 0.5098 / CI [0.5039, 0.5166] (n=8) |
| MLP | unknown_combinations | 0.6934 ± 0.03816 / 中央値 0.7031 / CI [0.6675, 0.7158] (n=8) |
| GRU64 | constant_sensor | 0.6846 ± 0.05938 / 中央値 0.6797 / CI [0.6475, 0.7222] (n=8) |
| GRU64 | correlated_noise | 0.8438 ± 0.165 / 中央値 0.8906 / CI [0.7222, 0.9233] (n=8) |
| GRU64 | delayed_sensor | 0.7031 ± 0.1169 / 中央値 0.6895 / CI [0.6265, 0.7759] (n=8) |
| GRU64 | dropout | 0.7495 ± 0.05317 / 中央値 0.7422 / CI [0.7148, 0.7832] (n=8) |
| GRU64 | long_delay | 0.3706 ± 0.117 / 中央値 0.3281 / CI [0.3286, 0.4536] (n=8) |
| GRU64 | noise_increase | 0.7588 ± 0.1544 / 中央値 0.8008 / CI [0.647, 0.8379] (n=8) |
| GRU64 | novel_scale | 0.5464 ± 0.232 / 中央値 0.6055 / CI [0.397, 0.6909] (n=8) |
| GRU64 | partial_inversion | 0.09131 ± 0.02624 / 中央値 0.08203 / CI [0.08203, 0.1099] (n=8) |
| GRU64 | resource_reduction | 0.8389 ± 0.08354 / 中央値 0.8496 / CI [0.7832, 0.8901] (n=8) |
| GRU64 | sensor_failure | 0.6846 ± 0.05938 / 中央値 0.6797 / CI [0.6475, 0.7222] (n=8) |
| GRU64 | sensor_inversion | 0.2378 ± 0.1502 / 中央値 0.3066 / CI [0.1318, 0.3257] (n=8) |
| GRU64 | sensor_permutation | 0.1328 ± 0.0673 / 中央値 0.1758 / CI [0.08789, 0.1685] (n=8) |
| GRU64 | stuck_at | 0.6846 ± 0.05938 / 中央値 0.6797 / CI [0.6475, 0.7222] (n=8) |
| GRU64 | unknown_combinations | 0.3145 ± 0.1569 / 中央値 0.2422 / CI [0.2207, 0.4219] (n=8) |
| GRU128 | constant_sensor | 0.7485 ± 0.03207 / 中央値 0.7363 / CI [0.7305, 0.7705] (n=8) |
| GRU128 | correlated_noise | 0.9248 ± 0.03404 / 中央値 0.9277 / CI [0.9019, 0.9458] (n=8) |
| GRU128 | delayed_sensor | 0.8232 ± 0.058 / 中央値 0.8516 / CI [0.7822, 0.8506] (n=8) |
| GRU128 | dropout | 0.793 ± 0.01683 / 中央値 0.7969 / CI [0.7817, 0.8032] (n=8) |
| GRU128 | long_delay | 0.5288 ± 0.1515 / 中央値 0.5879 / CI [0.4238, 0.624] (n=8) |
| GRU128 | noise_increase | 0.8647 ± 0.05861 / 中央値 0.8691 / CI [0.8232, 0.8979] (n=8) |
| GRU128 | novel_scale | 0.5259 ± 0.1967 / 中央値 0.5332 / CI [0.4058, 0.6519] (n=8) |
| GRU128 | partial_inversion | 0.23 ± 0.08441 / 中央値 0.2734 / CI [0.1655, 0.2764] (n=8) |
| GRU128 | resource_reduction | 0.9814 ± 0.05248 / 中央値 1 / CI [0.9443, 1] (n=8) |
| GRU128 | sensor_failure | 0.7485 ± 0.03207 / 中央値 0.7363 / CI [0.7305, 0.7705] (n=8) |
| GRU128 | sensor_inversion | 0.2358 ± 0.06516 / 中央値 0.1914 / CI [0.1997, 0.2769] (n=8) |
| GRU128 | sensor_permutation | 0.1138 ± 0.08028 / 中央値 0.1289 / CI [0.05908, 0.1611] (n=8) |
| GRU128 | stuck_at | 0.7485 ± 0.03207 / 中央値 0.7363 / CI [0.7305, 0.7705] (n=8) |
| GRU128 | unknown_combinations | 0.7241 ± 0.09632 / 中央値 0.7324 / CI [0.6636, 0.7852] (n=8) |

最弱の個別条件：MLP=sensor_permutation、GRU64=partial_inversion、GRU128=sensor_permutation。14 named conditionsだがsensor_failure/constant_sensor/stuck_atなどは同じmechanismを含む。重複列を独立な摂動と数えた単純平均を頑健性指標には使わない。response inverted/plausible_wrong、missing/contradictoryもCore観測が等価になり得る。 [ood_heatmap.png](ood_heatmap.png)

## 14. J72 end-to-end

health応答のAPI label=j72-30m、実parameter総数=150001152。API label中の30mを実parameter数と読み替えない。

| validation選択代表 | live calls | API成功 | parse成功 | 意味正解 | Core成功 | scripted Core成功 | 応答平均秒 |
|---|---|---|---|---|---|---|---|
| gru64-s2-B0 | 24 | 1 | 0 | 0 | 0.5417 | 1 | 1.368 |
| gru128-s6-B0 | 24 | 1 | 0 | 0 | 0.5417 | 1 | 1.375 |

実行層：HTTP→strict JSON parser→category/confidence/evidence_id→数値観測→Core hidden state→次行動。語彙的なA/B record抽出課題であり一般推論や自然言語理解を証明しない。J72代表はvalidationで選び、held-outで選び直さない。forced_gate_diagnosticがある場合、それはinterface診断であり学習gate成功へ合算しない。

| 代表 | 異常条件 | finite Core | downstream成功 | 証拠範囲 |
|---|---|---|---|---|
| gru64-s2-B0 | timeout | はい | 0.375 | controlled fixture |
| gru64-s2-B0 | http_503 | はい | 0.375 | controlled fixture |
| gru64-s2-B0 | malformed | はい | 0.375 | controlled fixture |
| gru64-s2-B0 | empty | はい | 0.375 | controlled fixture |
| gru64-s2-B0 | contradiction | はい | 0 | controlled fixture |
| gru64-s2-B0 | slow | はい | 1 | controlled fixture |
| gru128-s6-B0 | timeout | はい | 0.375 | controlled fixture |
| gru128-s6-B0 | http_503 | はい | 0.375 | controlled fixture |
| gru128-s6-B0 | malformed | はい | 0.375 | controlled fixture |
| gru128-s6-B0 | empty | はい | 0.375 | controlled fixture |
| gru128-s6-B0 | contradiction | はい | 0 | controlled fixture |
| gru128-s6-B0 | slow | はい | 1 | controlled fixture |

timeout/HTTP503/malformed/empty/contradiction/slowは決定論的transport fixtureであり、実サービスで同じ故障を起こした証明ではない。HTTPの実wall-clock秒と環境の固定step latencyも異なる。サービスを一時起動し元の状態へ復元した証拠はfinal_runtime_state.jsonを参照する。

## 15. 失敗例と未達

oracle_upper_bound: PARTIAL — 情報制約付きanalytic参照との比較。大域最適性・厳密な上界は証明していない。oracle gate後の行動には学習policyを使用する。

memory_temporal_ood_generalization: FAIL — 訓練47以外の事前指定8/16/24/40/80/160/320/640で評価。少なくとも一条件で両GRUの95% CI上限≤chance .5なら広い時刻転移はFAIL。非単調な破綻から単一memory horizonは定義しない。

j72_closed_loop: FAIL — 学習gateによるlive呼び出し、HTTP、strict parse、意味正解、Core downstreamを分離。forced-gate補助診断は学習gateの成功に含めない。

## 16. 統計

独立単位はtraining seedであり、3構成各8 seedを基本とする。episodeとstepはseed内の測定で、独立nやp値を増やさない。各scalarについてmean、sample SD、median、4000回のseed bootstrap percentile 95% CIを計算。paired比較は同じseedと同じ評価episode RNGを対応させ、zero差を除いたexact two-sided sign testを示す。CIは訓練seed変動だけを表し、全seedが共有する固定held-out環境からの母集団sampling不確実性を含まない。幅0のCIも確実性や一般化を保証しない。小標本のCIとp値を併記し、多数の副次比較はfamilywise補正をしていない探索的結果として扱う。主要behavioral PASSはn≥8、平均差正、CI下限>0、sign p≤.05とする。これは効果の一般性を保証しない。詳細な対応差は [report_statistics.json](report_statistics.json)。

## 17. 限界

合成binary fact、公開service条件、既知の物理affordance、固定deadline、1 episode 1call、decisionまで残る応答という制限がある。厳密なoracle最適性は未証明。time shuffleはfrozen baseline sensor tapeの非因果診断で、未来情報を前倒しし得るためH1/H3の因果根拠に使わない。個別maskには公開protocol channelの破壊が含まれ、そこでの性能低下をshortcut発見と即断しない。logical active connection数はcompute proxyであり、physical sparse speedupやFLOPs測定ではない。64-update pilotとprimaryは同一seedでも独立反復としてpoolしない。J72はA/B抽出のみであり大規模推論の品質は未評価。

## 18. 成功条件判定

| 条件 | 判定 | 根拠 |
|---|---|---|
| closed_loop_action_changes_future_observation | PASS | validation.jsonのE2テスト結果。個別環境テストの範囲に限定し、実サービス動作とは区別する。 |
| precall_ambiguity | PASS | 対象24 seed記録。no-call A/B履歴と行動のbit一致・balanced成功率≤0.5を照合。 |
| returned_information_used | PASS | GRU64/GRU128のcorrect−shuffled差（言語subset）、8 seed CIとexact sign test。 |
| learned_gate_better_than_never | PASS | 言語subsetでlearned−never、同一episode群。 |
| learned_gate_better_than_matched_random | PASS | 受理call数一致の確認とlearned−global matched random差。episode割当と時点を両方変えるため、純粋なtiming効果ではない。 |
| oracle_upper_bound | PARTIAL | 情報制約付きanalytic参照との比較。大域最適性・厳密な上界は証明していない。oracle gate後の行動には学習policyを使用する。 |
| leakage_audit | PASS | RNG stream分離、有限fingerprint、no-call A/B、strict memory、固定正規化、baseline hashを確認。未試験入力全体への無漏洩証明ではない。 |
| recurrent_memory_dependency | PASS | 既知の訓練delay=47におけるnormal−reset every step、GRU−MLP、cue A/Bの消失後観測bit一致・最終行動分岐の補足診断。未知delayへの転移とは分離する。 |
| memory_temporal_ood_generalization | FAIL | 訓練47以外の事前指定8/16/24/40/80/160/320/640で評価。少なくとも一条件で両GRUの95% CI上限≤chance .5なら広い時刻転移はFAIL。非単調な破綻から単一memory horizonは定義しない。 |
| ppo_causal_dissection | PASS | B1–B7各8 seed=True。同一parent hash検証=True。PPOが改善したこと自体を要求せず、退行を含めて因子別に報告する。 |
| paired_causal_forks | PASS | 同一env snapshotとmodel stateからCALL/NO CALL分岐。後続callを両群で無効化した単回強制call estimand。 |
| j72_closed_loop | FAIL | 学習gateによるlive呼び出し、HTTP、strict parse、意味正解、Core downstreamを分離。forced-gate補助診断は学習gateの成功に含めない。 |

実行完了=True、研究成功=FAIL。結果は [success_criteria.json](success_criteria.json) に機械可読で保存する。

## 19. 次phase判断

validation seed700001の選択score（scenario macro成功率 + 0.1×episode reward）の8 seed平均：MLP 0.799198、GRU64 1.077956、GRU128 1.101479。best Coreは GRU128。軽量候補GRU64は別に保持し、test最高seedで選び直さない。

このデータからK1進行を自動承認しない。returned information利用・never/matched random優位・memory依存・漏洩監査の不足があれば、その具体的な環境／学習問題を先に解消する。GRU64とGRU128がvalidationで同等なら軽量なGRU64を優先し、GRU128だけが明確に優れるならGRU128をbaselineとする。単一seedのtest最高値をbest Coreとは呼ばない。最終ユーザー確認は全実験・検証・commit後に行う。

## 20. 再現方法

保存されたsource manifest・checkpoint hash・config・RNG状態・package一覧を揃え、同じprotocol identityで実行する。評価identityが異なる再実行は新artifact namespaceへ保存する。

```bash
python -m experiments.k0_e2_active_info.evaluate --artifacts experiments/k0_e2_active_info/artifacts/primary --all --num-envs 256 --long-envs 64 --threads 4
for cp in experiments/k0_e2_active_info/artifacts/primary/runs/*-B0/imitation_final.pt experiments/k0_e2_active_info/artifacts/primary/runs/*-B8/ppo_final.pt; do
  python -m experiments.k0_e2_active_info.evaluate --artifacts experiments/k0_e2_active_info/artifacts/primary --checkpoint "$cp" --basic-only --threads 4
done
python -m experiments.k0_e2_active_info.trained_delay --artifacts experiments/k0_e2_active_info/artifacts/primary
python -m experiments.k0_e2_active_info.visualize --artifacts experiments/k0_e2_active_info/artifacts/primary
python -m experiments.k0_e2_active_info.report --artifacts experiments/k0_e2_active_info/artifacts/primary
```

--allはB0-best全評価とB1–B7-final basic評価を実行する。別loopでimitation-finalとB8-finalを補完し、trained_delayで既知時刻47の補助診断を再現する。再評価は保存checkpointを使い、既存K0-Eを変更しない。required plot：[voi_cost_curve.png](voi_cost_curve.png)、[voi_performance_curve.png](voi_performance_curve.png)、[reliability_curve.png](reliability_curve.png)、[latency_curve.png](latency_curve.png)、[policy_intervention.png](policy_intervention.png)、[response_ablation.png](response_ablation.png)、[retention_curve.png](retention_curve.png)、[ppo_ablation.png](ppo_ablation.png)、[ood_heatmap.png](ood_heatmap.png)、[pareto.png](pareto.png)。

## 21. 実行証跡と計算資源

| 項目 | 保存値 |
|---|---|
| 一時service元の停止状態へ復元 | はい |
| GPU compute process数 | 0 |
| 完了run数 | 88 |
| 非finite checkpoint数 | 0 |
| 学習transition総数 | 327155712 |
| critic追加transition | 1572864 |
| テストPASS数 | 51 |
| 追加subtest PASS数 | 16 |
| native GUI | PASS |
| baseline hash不変 | はい |
| baseline対象file数 | 394 |
| seed stream重複 | いいえ |
| 重複したexogenous episode件数 | 0 |
| CPU benchmark共通host数 | 1 |
| CPU thread設定 | 4 |
| E2 tests PASS | はい |
| PPO parent hash確認 | はい |

RTX3060/P100は別workerで、同じseedは全armで同じ物理GPUを使う。GPU utilization・VRAMの生測定はgpu_telemetry.jsonl、集計はresource_summary.jsonを参照する。training wall timeとcheckpoint情報はrun_evidence.jsonに保存する。CPU推論は同一host・batch1・4threadsでmean/median/p95を区別する。PyTorch allocated memoryをnvidia-smiの総VRAM使用量と呼ばない。GPU worker終了、一時J72 service復元、commit一覧は最終root監査の証跡で確認する。このreport generatorはネットワーク呼び出し、学習、service変更、commit/pushを行わない。

## 22. 最終自己レビュー

主実験88本・112評価行、各主要群8 seedを確認。 264 checkpointのSHA-256・stage・有限値を実体から検査し、全PPO枝の初期重み・親checkpoint・RNG・GPU対応が一致した。

K0-Eの394ファイルと既存best checkpoint24本は不変。J72一時サービスと既存推論サービスは停止状態、GPU学習workerは終了している。日本語GUIは実Macのアクセシビリティ状態と別途offscreen画像で確認した。

[最終実行状態](final_runtime_state.json)、[checkpoint監査](run_evidence.json)、[GUI描画検証](gui_verification.json)、[計算資源](resource_summary.json)を保存。B8とB3の成功率差は0で、追加KLが低LR単独より必要という証拠はない。

結果commit直前の実装commit系列（外部公開・pushなし）：

- `a3bcd54` feat: add isolated K0-E2 closed-loop information environment
- `b4cbce2` test: validate K0-E2 ambiguity and counterfactual dynamics
- `34e0260` feat: add paired K0-E2 imitation and PPO ablation training
- `6596dfa` feat: validate external language feedback and leakage boundaries
- `a8c4196` feat: add closed-loop J72 evaluation and bounded validation escalation
- `6469e38` feat: add K0-E2 interventions, causal evaluation, and plots
- `29683e8` feat: add validation-only selection of one stabilized PPO candidate
- `134f4ee` feat: add Japanese K0-E2 dashboard and closed-loop integration tests
- `475f1e7` docs: describe K0-E2 cognitive information acquisition protocol
- `719d29f` results: add checkpoint evidence audit and stabilized PPO plots
- `1f1f64f` test: separate trained-delay memory use from timing generalization
- `510dd2c` docs: generate audited K0-E2 findings and final validation report

結果を含む最終commitはGit履歴と最終回答で識別する。この追記は `python -m experiments.k0_e2_active_info.finalize --artifacts experiments/k0_e2_active_info/artifacts/primary` で再生成できる。
