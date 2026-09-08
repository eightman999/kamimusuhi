# 器官設計監査の文献台帳 — 2026-09-08

Status: **research evidence register / non-normative**  
調査日: **2026-09-08**  
対象: Kamimusuhi の設計不足を補うための認知科学・制御・記憶・分散系・セキュリティ・対話研究。

関連: [監査本体](../audits/2026-09-08-organ-design-audit.md) / [器官間契約と実装順序](../organ-contracts-and-implementation-plan.md)

## 読み方と調査限界

これは網羅的な systematic review や追試報告ではない。既存仕様を監査し、その不足に直接対応する一次論文を選んだ focused review である。検索結果の二次解説を技術的根拠には採用していない。2026年の論文も含むが、全分野の最新文献を漏れなく含むとは主張しない。

確認深度を区別する。

- **本文該当節**: 一次論文の HTML 本文で、採用に関係する方法・議論・限界の該当箇所を確認。論文全体の精読・実験再現を意味しない。
- **一次要旨**: 出版元、著者公開ページ、arXiv 等の要旨と書誌を確認。本文固有の数値・細部には依存しない。
- **書誌のみ**: 一次掲載情報は確認したが本文取得に失敗したもの。設計の系譜を示す参考であり、今回確認した実験的証拠には数えない。

以下の「適用」は **Kamimusuhi 側の設計提案** であって、引用論文が Kamimusuhi の正しさを証明するという意味ではない。論文の性能値を手元のハードウェア性能や合格 SLO に転記しない。

## R01 — 恒常性と動機

**Keramati, M. & Gutkin, B. (2014). Homeostatic reinforcement learning for integrating reward collection and physiological stability. eLife 3:e04811.**  
一次資料: <https://elifesciences.org/articles/04811>  
確認: **本文該当節**。内部状態・drive・報酬の関係、理論の前提と限界。

内部状態を行動価値に結び付ける計算モデルである。Kamimusuhi では、資源不足を単なる「疲れ」ラベルではなく、計測値→調整状態→資源配分へ接続する発想を借りる。生理的 setpoint を CPU 使用率にそのまま移植しない。人工的な自己維持欲求に、操作者の停止・予算・プライバシー制約を上書きする権限を与えない。対応: A05、A06、A13。

## R02 — 考え続ける価値と停止

**De Sabbata, N. et al. Rational Metareasoning for Large Language Models. arXiv:2410.05563v3, 2025-06-23.**  
一次資料: <https://arxiv.org/html/2410.05563v3>  
確認: **本文該当節**。§2、§3、§7。

計算の期待利益と費用を比較する Value of Computation を、推論量の学習に利用する。Kamimusuhi では追加推論・検索・委譲の費用を、成功見込みの改善と比較する研究方針に対応する。ただし論文自身が agentic setting と tool cost への拡張を未検証としている。最初は上限付き決定規則を使い、学習ルータの優越性は別途測る。対応: A05、A07。

## R03 — 信念の依存関係

**Doyle, J. (1979). A Truth Maintenance System. Artificial Intelligence 12(3), 231–272.**  
一次掲載: <https://doi.org/10.1016/0004-3702(79)90008-0>  
確認: **書誌のみ**。出版社・MIT 書誌を検索で確認したが、今回の本文取得は失敗。

信念維持システムという既存研究の系譜を指す参照。今回提案する「根拠の撤回が派生信念へ伝播する依存グラフ」は、まず工学的要求として定義する。Doyle の個別アルゴリズムを精読・再現済みとはしない。依存関係を保つことと、根拠自体が真であることも別である。対応: A03、A04。

## R04 — 出来事の分節

**Zacks, J. M., Speer, N. K., Swallow, K. M., Braver, T. S. & Reynolds, J. R. (2007). Event Perception: A Mind/Brain Perspective. Psychological Bulletin 133(2), 273–293.**  
一次本文: <https://pmc.ncbi.nlm.nih.gov/articles/PMC2852534/>  
確認: **本文該当節**。Event Segmentation Theory、prediction error、multiple timescales、議論。

連続入力を出来事へ区切る理論を扱う。Kamimusuhi では、会話セッションや固定 token chunk と「一つの経験」を同一視しない設計を支える。最初は明示的な話題・タスク・参加者・時間の変化による分節と比較する。人間のイベント知覚に関する理論であり、予測誤差を置くだけで人工的経験や意識が成立する証拠ではない。対応: A02、A08、A09。

## R05 — 行動の予測モデル

**Hafner, D., Pasukonis, J., Ba, J. & Lillicrap, T. (2025). Mastering diverse control tasks through world models. Nature.**  
一次掲載: <https://doi.org/10.1038/s41586-025-08744-2>  
著者 preprint 系譜: <https://arxiv.org/abs/2301.04104>  
確認: **一次要旨・書誌**。Nature 本文は今回取得できなかった。preprint と掲載版の題名・年を混同しない。

Dreamer 系の予測・想像による制御は、実際の結果と予想結果を分離する参考になる。Kamimusuhi の初期 world state は、対象・状態・観測・予定効果を保持する小さな構造化表現でよい。汎用の潜在世界モデルを v0.1 の必須条件にはしない。制御タスクでの結果は、人格連続性や社会的判断を保証しない。対応: A08、A10。

## R06 — 確信度の校正

**Guo, C., Pleiss, G., Sun, Y. & Weinberger, K. Q. (2017). On Calibration of Modern Neural Networks. ICML / PMLR 70.**  
一次資料: <https://proceedings.mlr.press/v70/guo17a.html>  
確認: **一次要旨**。

分類モデルの確率と実際の正解率のずれを扱う。器官が返す `confidence` を、そのまま意味的な正しさの確率とみなさない方針に対応する。タスク別の保留率・誤り率・確率校正を実測する。分類器向け temperature scaling が自由生成 LLM の発話確信度を自動的に校正するわけではない。対応: A03、A07、A15。

## R07 — 速い記憶と遅い学習

**McClelland, J. L., McNaughton, B. L. & O'Reilly, R. C. (1995). Why there are complementary learning systems in the hippocampus and neocortex: Insights from the successes and failures of connectionist models of learning and memory. Psychological Review.**  
一次論文の要旨・書誌: <https://pubmed.ncbi.nlm.nih.gov/7624455/>  
確認: **一次要旨**。

速い経験記録と遅い構造学習の分業を、既存 Kamimusuhi の CLS 方針の基礎として再確認した。今回の追加点は、新しい記憶層の提案ではなく、学習対象を根拠・同意・評価分割付きで凍結する契約である。再生を重ねた自己生成文を独立した経験として数えない。対応: A04、A16、A17。

## R08 — 更新・時間・知らないことを含む記憶評価

**Wu, D. et al. (2024). LongMemEval: Benchmarking Chat Assistants on Long-Term Interactive Memory. arXiv:2410.10813v1.**  
一次資料: <https://arxiv.org/html/2410.10813v1>  
確認: **本文該当節・要旨**。評価能力と構成。

情報抽出、複数セッション、時間推論、知識更新、回答保留を分けて評価する。Kamimusuhi では、記憶の単発 recall だけでなく、訂正・時点・不明を評価する設計に使う。benchmark の利用時には版と dataset hash を固定する。これは canonical head、削除伝播、クラッシュ復旧を検査する benchmark ではない。対応: A03、A09、A18。

## R09 — 長期会話の評価

**Maharana, A. et al. (2024). Evaluating Very Long-Term Conversational Memory of LLM Agents. arXiv:2402.17753.**  
一次資料: <https://arxiv.org/abs/2402.17753>  
確認: **一次要旨**。

LoCoMo は、長期の複数セッションにまたがる記憶評価の参考。Kamimusuhi では生の会話・要約・イベント表現の比較に利用する。長期会話で答えられることと、個体の権限・系譜が正しいことは別に測る。評価データを学習や反省用の経験へ混入させない。対応: A09、A15、A18。

## R10 — 分散した正典の合意

**Ongaro, D. & Ousterhout, J. (2014). In Search of an Understandable Consensus Algorithm. USENIX ATC.**  
一次資料: <https://www.usenix.org/conference/atc14/technical-sessions/presentation/ongaro>  
確認: **一次要旨・掲載情報**。

複製状態機械と合意の研究を、複数 writer の authority 設計の基礎に置く。初期 Kamimusuhi では単一 writer と条件付き transaction から始め、分散合意を自作しない。Raft を採用しただけで、悪意あるノード・個体同一性・外部 API の重複実行まで解決したとはしない。対応: A01、A10、A14。

## R11 — 学習制御器の外側の安全枠

**Alshiekh, M. et al. (2018). Safe Reinforcement Learning via Shielding. AAAI.**  
一次資料: <https://ojs.aaai.org/index.php/AAAI/article/view/11797>  
preprint: <https://arxiv.org/abs/1708.08611>  
確認: **一次要旨**。

学習器の選択を外側の安全仕様で制限する shield を扱う。既存 PNL の R0 > R1 という境界を具体化する参考になる。安全性は仕様・環境モデル・観測可能性などの前提に依存し、任意のロボット動作を無条件で安全にする魔法ではない。対応: A06、A11、A13。

## R12 — 命令権限と外来データの分離

**Debenedetti, E. et al. Defeating Prompt Injections by Design. arXiv:2503.18813v2, 2025-06-24.**  
一次資料: <https://arxiv.org/html/2503.18813v2>  
確認: **本文該当節**。CaMeL の capability、policy、§9 の限界。

信頼する制御と非信頼データを分け、値に付く provenance・許可読者等を実行時に扱う。Kamimusuhi では tool、記憶、Library、外部モデル、可変 skill を同じ権限体系で扱う参考になる。ただし誤った要約など、制御・データフローを破らない内容攻撃は解決対象外。出典管理と「正しい信念」の検証は別系統にする。対応: A03、A11、A13。

## R13 — 永続記憶を通じた攻撃

**Dong, S. et al. Memory Injection Attacks on LLM Agents via Query-Only Interaction. arXiv:2503.03704v4, 2025-12-10.**  
一次資料: <https://arxiv.org/html/2503.03704v4>  
旧版題名: A Practical Memory Injection Attack against LLM Agents.  
確認: **本文掲載情報・要旨**。

MINJA は、通常の対話経路から記憶を汚染し、後続の行動を誘導する脅威を示す。Kamimusuhi では「DB への直接侵入だけを防げばよい」という threat model を退ける根拠となる。攻撃成功率を全環境へ一般化せず、権限昇格・根拠ロンダリング・他者記憶混入の防御試験へ落とす。対応: A03、A11、A18。

## R14 — 成功を繰り返せるか

**Yao, S., Shinn, N., Razavi, P. & Narasimhan, K. (2024). τ-bench: A Benchmark for Tool-Agent-User Interaction in Real-World Domains. arXiv:2406.12045.**  
一次資料: <https://arxiv.org/abs/2406.12045>  
確認: **一次要旨**。

最終 DB 状態と policy に照らした評価、および繰返し信頼性の `pass^k` を提案する。Kamimusuhi では「一度うまく答えた」ではなく、副作用の最終状態と反復結果を測る。有限回の全成功は、無限期間の安全性の証明ではない。シミュレーションと実 API のクラッシュ挙動も分離する。対応: A10、A18。

## R15 — 音声生成と同時聴取

**Défossez, A. et al. (2024). Moshi: a speech-text foundation model for real-time dialogue. arXiv:2410.00037.**  
一次資料: <https://arxiv.org/abs/2410.00037>  
確認: **一次要旨**。

利用者側とシステム側の音声ストリームを扱う full-duplex 設計の一次参照。Kamimusuhi では TTS の速さだけでなく、聞き続けること・割込み・自己音声分離を独立要求にする。論文の latency は固有の実装条件での報告であり、Kamimusuhi の実測値ではない。対応: A12。

## R16 — 「間」の改善と意味・安全の回帰

**Ohashi, A., Zeghidour, N., Défossez, A. & Kharitonov, E. (2026). Multi-Faceted Interactivity Alignment in Full-Duplex Speech Models. arXiv:2606.11167v1, 2026-06-09.**  
一次資料: <https://arxiv.org/html/2606.11167v1>  
確認: **本文該当節**。§1、方法の概要、限界、Appendix D.2。

pause、turn-taking、backchannel、user interruption の四軸を分けて最適化・評価する。特に、Fisher による PersonaPlex の調整で、協力的な相槌傾向と安全課題が衝突した例を報告している。Kamimusuhi では timing 改善と同時に、誤同意・意味の保持・拒否・許可の捏造を回帰試験に入れる。全モデルに同じ劣化が起こるという主張ではない。対応: A12、A15、A18。

## R17 — 誰が何を知っているか

**Kim, H. et al. (2023). FANToM: A Benchmark for Stress-testing Machine Theory of Mind in Interactions. EMNLP.**  
一次資料: <https://aclanthology.org/2023.emnlp-main.890/>  
確認: **一次要旨・掲載情報**。

情報非対称な会話で他者の知識状態を問う benchmark。Kamimusuhi では、個体の全記憶を全参加者の共通知識と誤認しないテストへ適用する。「再生した」は「相手が聞いた・理解した・同意した」の証拠ではない、という区別は今回の工学的設計提案である。対応: A12、A15。

## R18 — モデルに入った情報の忘却

**Bourtoule, L. et al. (2021). Machine Unlearning. IEEE Symposium on Security and Privacy. arXiv:1912.03817v3.**  
一次資料: <https://arxiv.org/html/1912.03817v3>  
確認: **本文該当節**。§IV の SISA と評価上の trade-off。

学習影響を分割・隔離し、削除対象を含む部分を再学習する構造を扱う。Kamimusuhi では、記憶レコード削除とモデルからの忘却を同一視しない根拠になる。SISA を Persona Core にそのまま採用する決定ではない。データ追跡、学習前 checkpoint、再学習可能性を先に用意し、未対応の削除範囲は明示する。対応: A04、A16、A17。

## R19 — 勾配は匿名情報とは限らない

**Zhu, L., Liu, Z. & Han, S. (2019). Deep Leakage from Gradients. NeurIPS.**  
一次資料: <https://papers.nips.cc/paper/9617-deep-leakage-from-gradients>  
確認: **一次要旨**。

共有勾配から学習入力を復元できる条件を示す。個体群で重み差分や勾配を共有するだけなら安全、という仮定を置かない理由になる。ただし「任意の LoRA から任意の会話を必ず復元できる」と拡大解釈しない。差分ごとの threat model、同意、非私的データによる再現、漏洩評価が必要。対応: A17。

## R20 — 時間的にまとまった技能

**Sutton, R. S., Precup, D. & Singh, S. (1999). Between MDPs and semi-MDPs: A framework for temporal abstraction in reinforcement learning. Artificial Intelligence 112, 181–211.**  
一次掲載: <https://doi.org/10.1016/S0004-3702(99)00052-1>  
確認: **書誌のみ**。出版社本文の取得は失敗。

時間的抽象化を持つ技能の研究系譜への参照。今回の skill 契約では、開始条件・動作・終了条件を別々に持たせるが、options の学習アルゴリズムを導入・追試したとはしない。実装は allowlist 済み手続きと決定的状態機械から始める。対応: A05、A10、A16。

## R21 — 記憶攻撃評価の条件依存性

**Devarangadi Sunil, B. et al. (2026). Memory Poisoning Attack and Defense on Memory Based LLM-Agents. arXiv:2601.05504, 2026-01-09.**  
一次資料: <https://arxiv.org/abs/2601.05504>  
確認: **一次要旨**。preprint として扱う。

EHR agent の記憶攻撃について、既存の正常な記憶や検索条件により効果が変わり、強すぎる防御が正常記憶も拒否し得ることを報告する。Kamimusuhi では攻撃成功率だけでなく正常処理の成功率・誤拒否率・記憶量ごとの差を測る。医療 agent での結果を一般的な防御保証へ拡張しない。対応: A11、A18。

## 採用判断の要約

**すぐ契約へ反映するもの**は、根拠と権限の分離、時間・更新・回答保留を含む記憶評価、外部作用の状態確認、反復信頼性、音声の四軸評価、学習データまで含む削除範囲の明示である。

**比較実験として扱うもの**は、学習された metareasoning、homeostatic RL、予測誤差によるイベント分節、latent world model、SISA、学習型 full-duplex controller である。名称が魅力的だから production dependency にしない。

**この台帳が保証しないもの**は、意識・生命・人格同一性の成立、全攻撃への耐性、任意の個人情報の完全忘却、既存ハードウェアでの低レイテンシ達成である。これらのうち工学的に測れる部分は、監査本体と実装計画の試験へ個別に分解する。
