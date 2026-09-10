# K0-F 事前登録プロトコル

版 `k0-f-protocol-v1`。K0-E/E2は保存し、独立ディレクトリ・branchで実施する。実測収集の前に固定し、pilotは配線・処理量の較正に限る。testの結果に合わせた成功条件・学習予算の変更を禁止する。

## 主張と検証層

正しい現在の実機身体情報が資源選択の効用を改善するかを検証する。観測記録、測定済みコスト行列による方策評価、凍結方策からの実ジョブ実行を区別する。matrix multiplicationは実際の計算ジョブだが言語理解能力の代理ではない。実LLM workloadは独立したrecord-only区間で収集する。

## 固定条件

- real telemetry: Mac / master、原則1Hz。raw / aligned / normalized frame / policy inputを別保存。
- frameは20float、個別mask20、欠損はneutral値とmask=0を対にする。正規化versionとhashを保存。
- 3actions: RUN_CPU / RUN_RTX3060 / RUN_P100。不要なWAITや言語呼出actionは追加しない。
- taskは固定4種類の行列乗算。特徴はdimension/3072, repetitions/20, deadline/5, precision flag。seed/task/workloadID・action別コスト・絶対timestampは入力しない。
- 実測latencyにはjob dispatchと同期完了までを含む。同じtaskで3actionをランダム順に測定する。1回ずつの実測行列は同時反実仮想ではなく短い区間のpaired測定であり、この制約を明記する。
- teacherと評価utility: `1-min(latency_seconds/deadline_seconds,2)`、failure時-1。deadline超過率・latency・failureを別記録。安全上OOMを起こすteacherは用いない。
- 48 workload blocks、各20秒を基本とする。train24 / validation8 / test16、block単位で分離。split境界は35秒record-onlyを挿入し、STALE30秒参照も他splitのtask区間に入らない。各splitにidle/cpu/RTX/P100/dual/mixed/disk/networkを含む。task順・resource測定順は固定seedから生成し保存。収集に遅延があればblockを延長するが件数を増やして有意差を探さない。
- 独立単位はtraining seed0–7。GRU128 BODYと別学習BLIND。primaryは同じBODY checkpointへのBODY/BLIND/SHUFFLED/STALE介入。別学習BLINDとの差はsecondaryとする。
- sequence historyは1–8、固定step番号・block開始からの時刻を入力しない。STALEは実timestampで30秒前、欠損もmaskで表現。古い内容のstaleness channelには現在までの経過30秒を加算し、既知の古さを正しく通知する（age-aware STALE）。SHUFFLEDは同split内の異なるepisodeから取得、task/cost順を保持。
- imitation supervised learningのみ。全actionの実測feedbackがあるfull-information taskなので、追加DAggerで新規状態を作る必要性を評価し、不要なら実施しない。PPOは実施しない。
- probeはtrain-onlyfit/scaling、validationでBODY MAEがBLINDより10%以上改善すればpolicyへ進む。改善しなければtestを開かずsensor設計を点検、変更時は理由・旧protocol・旧pilotを保全する。
- policy checkpointはvalidation utility最大で選択。best/finalを両方保存する。学習の予算とoptimizerはpolicy contractでtest開封前に固定。

## 介入・統計

BODY vs BLIND/SHUFFLED/STALEは8seedのpaired utility差。mean>0、seed bootstrap95%CI下限>0、両側exactsign p<=.05を各primary gateに使う。seedごとのmean/SD/median/CIと原データを保存。telemetry点数・episode数を独立nにしない。seed CIは共通heldout workload下の学習seed不確実性を表し、一般的な機械環境へのCIではない。

counterfactualは同task・同hidden・同historyで最終bodyだけを交換し、交換先の実測actionコストでmatched actionと元の固定actionのutilityを比較する。action変化だけではPASSにしない。live replayは同じ凍結policyとpaired workload/task順にBODY/BLIND/SHUFFLED/STALEを実行し、実測時間・結果・policy入力を保存する。測定済みコスト再生とlive結果を混同しない。

必須temporal OOD: sampling interval(0.5/2秒相当)、body delay、network jitter、sensor dropout、stale、task timing。必須sensor OOD: noise/dropout/constant/delay/scaling/GPU unavailable/Mac unavailable/network latency増加/inversion/permutation。観測に対するcontrolled synthetic perturbationと実hardware故障を区別し、実故障は起こさない。

## 安全・終了

GPU背景処理duty<=50%、matrices<256MiB/device、CPU背景<=2threads。GPU>=75℃または観測不能なら背景処理停止。主収集側でもthermal/load監視。有限duration、全child PIDを所有してfinallyで終了。メモリ割当失敗を意図的に誘発しない。disk workloadは専用workディレクトリ内の小さい一時ファイルのみ。

実験前後のservices・compute PID・thermal/loadを保存し、今回のsensor/workload/workerのみ停止。既存service状態を復元。private接続情報は既存Git除外設定から参照し、raw/config/reportに記録しない。commitのみ、pushしない。全実装・実験・報告・cleanup後に次phase承認を一度だけ求める。

## 成績を開く前の監査補記

独立レビューで、SHUFFLEDがdonorの履歴長をそのまま使うとGRUの更新回数まで変わる交絡を発見した。学習・probe・heldout評価前に、同じ履歴長かつ異なるblockでの全単射shuffleへ修正する。raw収集と測定済みcostは変更しない。`prepare_dataset.py` が保存済み過去frameから各blockの8decisionに履歴長1〜8を各1回、固定seed110910+block順番で割り当て、`policy_dataset.jsonl` を別保存する。各split・各履歴長に複数blockを確保し、shuffleでtask順・GRU更新回数・body履歴周辺分布を保つ。長さの選定には成績・cost・workload labelを一切使わない。

live確認では凍結GRU128の8seedと4入力介入、および独立学習BLINDを、8種類の実workload下で動かす。同じtask/seedの5modeは同じ直前bodyを基点に実行順を無作為化する。raw telemetryは継続し、新規jobを選択された資源で実行する。live SHUFFLEDのdonorは同じ履歴長の保存済み別episodeの実測body（無作為抽出、cost/labelは使わない）。live主判定にはBODY対BLINDとBODY対独立学習BLINDのpaired utility改善を用いる。新規live結果を主収集の実測cost再生と別表示する。

## 収集中断と再取得

初回48block収集は24学習block後、Macのsleepに伴う約805秒のwall-clock空白（source monotonic増分は約1秒）によりmasterがtelemetry stalenessを検出して安全停止した。結果評価・学習は未実施。失敗記録は`artifacts/primary`と`primary_mac`に保存し、独立nへ加算しない。実験processの生存期間だけ`caffeinate -i -w PID`でidle sleepを抑制し、終了時に解放する。fresh `primary_v2`で同じ48block/task/seed/protocolを再取得する。時刻空白を補間で埋めず、成功条件・heldout定義は変更しない。
