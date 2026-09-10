# K0-F Machine Interoception / Minimal Embodiment 結果報告

## 1. 結論

**研究成功: FAIL。実施完了: 確認済み。**

**本学習・ablation・OOD・liveは、予測probe不合格後の探索的診断（exploratory_after_failed_probe）である。後続比較が良好でも研究全体FAILを固定し、確証的な研究成功へ昇格しない。**

未達・未検証 gate: D_BODY_vs_SHUFFLED、G_live_BODY_vs_TRAINED_BLIND、probe_validation_gate、confirmatory_scope。最良 Core は validation の seed 平均だけで選定し、GRU128。

実測テレメトリーの取得、保存済み実測費用による再生評価、凍結方策が選んだ資源で新規に実行した実ジョブを別の証拠層として示す。GUI の完成を研究成功に含めない。

| paired 比較 | seed n | 平均差 | 標本 SD | 中央値 | 95% CI | exact sign p | 判定 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| GRU128_BODY_vs_BLIND | 8 | 0.0368353 | 0.00386042 | 0.0373916 | [0.0344751, 0.0395338] | 0.0078125 | PASS |
| GRU128_BODY_vs_SHUFFLED | 8 | 0.0150611 | 0.0140646 | 0.0136565 | [0.00606973, 0.0242751] | 0.0703125 | FAIL / 未達 |
| GRU128_BODY_vs_STALE | 8 | 0.0807716 | 0.0714602 | 0.0403134 | [0.0379397, 0.130395] | 0.0078125 | PASS |

新規実ジョブの主確認:

| paired 比較 | seed n | 平均差 | 標本 SD | 中央値 | 95% CI | exact sign p | 判定 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| BODY_vs_BLIND | 8 | 0.0923767 | 0.0726478 | 0.0896186 | [0.0457012, 0.140826] | 0.0078125 | PASS |
| BODY_vs_TRAINED_BLIND | 8 | 0.0225021 | 0.0436296 | 0.00952242 | [-0.00673302, 0.050472] | 0.289062 | FAIL / 未達 |

## 2. 研究質問

現在の正しい機械身体情報を持つ非言語 Core が、身体情報を持たない Core より、資源選択と下流の task utility を改善するか。必要な因果鎖は「正しい身体情報 → 意思決定の変化 → 下流効用の改善」。温度分類や行動変化だけでは成功としない。

## 3. 実装

独立 namespace `experiments/k0_f_interoception` に、Swift Mac sensor、Linux Python sensor、SSH private transport、timestamp 整列、20次元正規化、44次元 policy 入力、GRU policy、介入評価、閲覧専用日本語 GUI を実装した。K0-E/E2 は入力互換や checkpoint の保全対象とし、既存実験を上書きしない。

GUI は保存済み raw 値、mask、取得時 age、quality、Core action/hidden norm、温度・使用率・VRAM・メモリー圧・RTT・行動時系列を表示する。観測未取得・未評価を正常値や成功値で補完しない。

検証結果 artifact: {"gui_native":"AX and screenshot sensor/frame/current action rendering verified; graph tab verified during recording","independent_audit":{"best_final_separated":true,"checkpoint_count":32,"checkpoint_hash_finite_checks":true,"confirmatory_eligible":false,"counterfactual_pairs_checked":30720,"counterfactual_same_history_hidden_task_final_body_only":true,"errors":[],"experiment_scope":"exploratory_after_failed_probe","initial_weights_independently_reconstructed":8,"live_actions_recomputed_from_inputs":true,"live_checkpoint_hashes_checked":16,"live_failures":0,"live_jobs_checked":1280,"live_random_order_reconstructed":true,"live_same_base_groups":256,"live_seed_mode_summaries":40,"live_statistics_independently_recomputed":true,"max_first_row_checksum_error_against_independent_double":7.76e-07,"ood_rows":136,"paired_initial_hash_matches":8,"policy_input_trace_rows_checked":5120,"replay_statistics_independently_recomputed":true,"schema_version":"k0-f-independent-policy-audit-v1","source_commit":"0fb0603","training_epoch_records_finite":2560,"whole_matrix_numerical_correctness_verified":false},"plots":{"all_real_artifact_data_present":true,"count":10,"visual_and_hash_qa_pass":true},"real_hardware":{"action_measurements":1152,"existing_j72_generation_requests":34,"interruption_smoke_cleanup":true,"live_frozen_policy_jobs":1280,"paired_cost_recording_decisions":384},"source_commit":"0fb0603f3872aed2dbbf1b045769d0dc4dcb37a4","test_policy":"single fixed evaluation, no post-test training or tuning","test_probe":"not evaluated; failed validation gate preserved","unit_tests":{"local_gui_offscreen":{"environment":"existing PyQt5/pyqtgraph macOS","pass":true,"tests":9},"remote_non_gui":{"environment":"Python/PyTorch CPU on master","pass":true,"tests":46},"total":55}}

## 4. Mac sensors

| 指標 | 値 |
| --- | --- |
| real samples | 1087 |
| 取得時間 秒 | 1086.01 |
| 最大間隔 秒 | 1.00928 |
| 時刻単調増加 | PASS |

Foundation thermalState、Mach CPU/VM、IOKit 電源、sysctl swap、ICMP RTT を privilege 不要で取得する。Mac の生温度・fan は安定公開 API がないため任意・未取得。熱圧は nominal/fair/serious/critical を 0..1 に encode。memory pressure は VM 使用圧 proxy で OS の pressure event そのものではない。

| 生 sensor / 単位はキー末尾 | 有効 sample | 記述平均 | 最小 | 最大 |
| --- | --- | --- | --- | --- |
| thermal_pressure | 1087 | 0 | 0 | 0 |
| cpu_utilization | 1086 | 0.152193 | 0.0294118 | 0.424242 |
| memory_pressure | 1087 | 0.604915 | 0.586145 | 0.632124 |
| battery_fraction | 1087 | 0.756743 | 0.74 | 0.77 |
| network_rtt_ms | 1084 | 55.2692 | 12.756 | 494.746 |
| network_loss | 1087 | 0.00183993 | 0 | 1 |
| daemon_cpu_fraction | 1086 | 0.007879 | 0.00274336 | 0.0132359 |
| daemon_rss_bytes | 1087 | 1.62585e+07 | 7.29088e+06 | 2.21676e+07 |

## 5. llm_master sensors

| 指標 | 値 |
| --- | --- |
| real samples | 1085 |
| 取得時間 秒 | 1084 |
| 最大間隔 秒 | 1.00266 |
| 時刻単調増加 | PASS |

/proc、sysfs、nvidia-smi から CPU 温度・使用率・iowait・RAM・I/O・RTX3060/P100 の温度、利用率、VRAM、電力を取得する。Mac→中央と中央→Mac の RTT は別 sensor。取得できない向きの値は欠損として残す。

| 生 sensor / 単位はキー末尾 | 有効 sample | 記述平均 | 最小 | 最大 |
| --- | --- | --- | --- | --- |
| cpu_temperature_c | 1085 | 39.1134 | 33 | 67 |
| cpu_utilization | 1085 | 0.0423289 | 0 | 0.225605 |
| memory_pressure | 1085 | 0.176965 | 0.109967 | 0.195821 |
| io_pressure | 1085 | 0.000172166 | 0 | 0.0046 |
| rtx3060_temperature_c | 1085 | 51.1677 | 41 | 65 |
| rtx3060_utilization | 1085 | 0.153281 | 0 | 0.75 |
| rtx3060_power_w | 1085 | 58.1993 | 14.78 | 113.16 |
| p100_temperature_c | 1085 | 40.2664 | 37 | 45 |
| p100_utilization | 1085 | 0.104645 | 0 | 0.6 |
| p100_power_w | 1085 | 38.972 | 24.21 | 121.17 |
| daemon_cpu_fraction | 1084 | 0.0473794 | 0.0189258 | 0.0718659 |
| daemon_rss_bytes | 1085 | 2.38674e+07 | 2.38674e+07 | 2.38674e+07 |

## 6. InteroceptiveFrame

| 固定 frame 監査 | 値 |
| --- | --- |
| frames | 1081 |
| valid_fixed_frames | 1081 |
| all_valid | PASS |
| normalization_identities | ["fa4ccb3d405ca670bbfad8b3015fdeb733848a7bf69d308b6df8d617d23065f3"] |
| schema_versions | ["k0f.frame.v1"] |
| mean_availability | 0.999949 |

20値 + 20 mask を保存し、task 4値と合わせて44 floatを policyへ渡す。欠損値0には必ずmask0を伴わせる。sensorごとのage/quality、source/receipt timestamp、sequence、正規化identityを保存し、raw/aligned/frame/policy-inputを別ファイルにする。心理ラベル・workload名・絶対時刻・cost・teacherをpolicy入力へ入れない。

正規化設定: {"clock_future_tolerance_s":2.0,"frame_names":["master_cpu_thermal","master_cpu_busy","master_ram_pressure","master_io_pressure","rtx3060_thermal","rtx3060_compute_busy","rtx3060_vram_pressure","rtx3060_power_pressure","p100_thermal","p100_compute_busy","p100_vram_pressure","p100_power_pressure","mac_thermal","mac_cpu_busy","mac_memory_pressure","mac_power_pressure","network_latency","network_loss","body_staleness","body_availability"],"frame_schema":"k0f.frame.v1","max_age_s":60.0,"missing_value":0.0,"policy_layout":"20-values-then-20-mask","raw_schema":"k0f.raw.v1","rtt_scale_ms":1000.0,"staleness_scale_s":60.0,"temperature_high_c":85.0,"temperature_low_c":30.0,"version":"k0f.normalize.v1"}

## 7. workload

保存 workload block 数: 48。split件数: {"test":16,"train":24,"validation":8}。

事前計画は48 block、train24 / validation8 / test16。idle、CPU、RTX3060、P100、dual GPU、mixed、disk I/O、network transferを含み、split境界に35秒の記録区間を置く。固定4種類の行列乗算を CPU/RTX3060/P100 で無作為順に測定する。計算 job は実処理だが言語理解の代理ではない。実LLM workloadのrecord-only証拠は別収集・別artifactで確認する。

1回ずつの action別測定は短時間のpaired測定であり、同時の物理反実仮想ではない。収集 sample 数を独立実験数と数えない。

初回primary収集は24block後のMac sleep（壁時計約805秒、monotonic約1秒）により安全停止した。中断記録を保持し、driver期間中のidle-sleep抑制を追加して同条件のprimary_v2を再取得した。初回中断データは今回のprimary集計・独立nに混ぜない。

取得試行の監査記録: [{"artifact_directory":"primary","collection_complete":false,"included_in_primary_data":false,"reason":"First attempt interrupted by Mac idle sleep; preserved, never pooled"},{"artifact_directory":"primary_v2","collection_complete":true,"included_in_primary_data":true,"reason":"Fresh fixed-protocol recording with process-scoped idle sleep prevention"}]

既存J72による実LLMの独立record-only区間:

| workload | model | source | 開始 | 終了 | request数 | record-only | PID | 終了コード |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| rtx3060_inference | existing_j72 | real | 2026-09-10T11:52:09+00:00 | 2026-09-10T11:52:34+00:00 | 17 | はい | 284432 | 0 |
| p100_inference | existing_j72 | real | 2026-09-10T11:52:39+00:00 | 2026-09-10T11:53:03+00:00 | 17 | はい | 284704 | 0 |

この実LLM区間はtelemetry取得の観測証拠として別保存する。primary_v2の学習・validation・held-out・seed統計には混ぜず、行列計算policyの言語能力評価とは扱わない。

## 8. prediction probe

ridge λ=10、特徴標準化・target SDはtrainだけでfit。初回v1はBODY 1.323946 / BLIND 0.607069でFAIL。I/O特徴のtrain SDが約0.00005894と微小で、validation変動が最大62.59 SDへ増幅されていた。test成績を見ずにbody特徴のSD下限0.05だけを一度修正し、target・λ・10%gateを維持したv2もFAILだった。未来10秒のGPU使用率と、次jobの実測最小完了時間を区別する。validationで有効非定数targetが2種類以上かつ BODY の正規化MAEが BLIND より10%以上低いことを学習開始 gate とする。

| validation gate | 値 |
| --- | --- |
| PASS | FAIL |
| 相対 MAE 改善 | -0.00166508 |
| target | ["future_rtx3060_util","future_p100_util","next_job_latency_seconds"] |
| BODY / BLIND / SHUFFLED / STALE MAE | {"BLIND":0.6070692256244671,"BODY":0.6080800418283293,"SHUFFLED":0.5578533391981672,"STALE":0.78230046519274} |
| test probe 実施 | 未実施（validation gate未達） |

validation gate が通っていないため、後続policy・ablation・OOD・liveは探索的診断として区別する。v1不合格を保存し、標準化SDのfloorを0.05へ修正した一回のv2も不合格。成功閾値を変更せず、追加probe探索で成功を探さない。後続成績にかかわらず研究全体はFAIL。

## 9. learning protocol

**探索的診断として実施。予測probe不合格後の実験であり、事前の学習開始gateを通過した確証試験ではない。研究全体FAILを固定する。別学習BLINDを含む全結果を表示し、良好な条件だけを採用しない。**

| 保存 training config | 値 |
| --- | --- |
| experiment_scope | exploratory_after_failed_probe |
| seeds | [0,1,2,3,4,5,6,7] |
| hidden_sizes | [128] |
| epochs | 160 |
| batch_size | 64 |
| learning_rate | 0.001 |
| device | cpu |
| selection | validation utility only |
| PPO | 未実施 |
| DAgger | 未実施 |
| source_commit | 0fb0603f3872aed2dbbf1b045769d0dc4dcb37a4 |

GRU128 BODY と独立 BLIND を同一seed・初期重み・task順で教師学習。utilityは `1-min(latency/deadline,2)`、実行失敗は-1。lossは teacher cross entropy + expected utility regret。1 episodeに1資源選択であり、replay actionは次rowを変えないので DAggerは実施しない。PPOも実施しない。validation utility最大の最初のbestと最終epochを別保存する。

| validation-only 選定 | 値 |
| --- | --- |
| architecture | GRU128 |
| selection | validation only |
| n_seeds | 8 |
| mean_validation_utility | 0.186123 |
| architectures_evaluated | ["GRU128"] |

GRU64: 未実施。軽量構造との優越性は未検証。

final checkpoint は `final_checkpoint_results.json` に別保存し、primaryをbestから置換しない。final比較の保存行数: 40

## 10. BODY vs BLIND

同じ BODY-best checkpoint への全body mask介入。情報除去による入力分布変化の影響があるため、独立学習BLINDとの差も別示する。

| paired 比較 | seed n | 平均差 | 標本 SD | 中央値 | 95% CI | exact sign p | 判定 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| GRU128_BODY_vs_BLIND | 8 | 0.0368353 | 0.00386042 | 0.0373916 | [0.0344751, 0.0395338] | 0.0078125 | PASS |

保存実測費用の再生評価:

| 条件 | seed n | 効用 | 遅延 秒 | 締切内成功率 | 実行失敗率 |
| --- | --- | --- | --- | --- | --- |
| BLIND | 8 | 0.188416 | 0.00832675 | 0.75 | 0 |
| BODY | 8 | 0.225251 | 0.00820527 | 0.759766 | 0 |
| SHUFFLED | 8 | 0.21019 | 0.00844385 | 0.749023 | 0 |
| STALE | 8 | 0.14448 | 0.0119024 | 0.694336 | 0 |
| TRAINED_BLIND（独立学習） | 8 | 0.213844 | 0.00826625 | 0.742188 | 0 |

独立学習した BLIND との secondary 比較:

| paired 比較 | seed n | 平均差 | 標本 SD | 中央値 | 95% CI | exact sign p | 判定 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| GRU128_BODY_vs_independently_trained_BLIND | 8 | 0.0114072 | 0.00615347 | 0.00988369 | [0.00783433, 0.0157551] | 0.0078125 | PASS |

凍結方策が選んだ資源で新規実行したジョブ:

| 条件 | seed n | 効用 | 遅延 秒 | 締切内成功率 | 実行失敗率 |
| --- | --- | --- | --- | --- | --- |
| BLIND | 8 | 0.43998 | 0.00748811 | 0.882812 | 0 |
| BODY | 8 | 0.532357 | 0.00727683 | 0.949219 | 0 |
| SHUFFLED | 8 | 0.509537 | 0.00737418 | 0.925781 | 0 |
| STALE | 8 | 0.453433 | 0.0102206 | 0.902344 | 0 |
| TRAINED_BLIND | 8 | 0.509855 | 0.00744139 | 0.921875 | 0 |

| paired 比較 | seed n | 平均差 | 標本 SD | 中央値 | 95% CI | exact sign p | 判定 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| BODY_vs_BLIND | 8 | 0.0923767 | 0.0726478 | 0.0896186 | [0.0457012, 0.140826] | 0.0078125 | PASS |
| BODY_vs_TRAINED_BLIND | 8 | 0.0225021 | 0.0436296 | 0.00952242 | [-0.00673302, 0.050472] | 0.289062 | FAIL / 未達 |

liveは同seed/task直前bodyを共通にし、5modeの実行順を無作為化する。実行時間は新規測定で、保存costの再使用ではない。共通bodyの取得から後続mode実行までの物理状態変化は残る。

## 11. BODY vs SHUFFLED

同split・同履歴長の別workload blockのbodyを全単射で交換し、task列・cost列・GRU更新回数・bodyの周辺分布を保持する。現在状態との対応を壊した対照。

| paired 比較 | seed n | 平均差 | 標本 SD | 中央値 | 95% CI | exact sign p | 判定 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| GRU128_BODY_vs_SHUFFLED | 8 | 0.0150611 | 0.0140646 | 0.0136565 | [0.00606973, 0.0242751] | 0.0703125 | FAIL / 未達 |

新規実ジョブでの補助比較:

| paired 比較 | seed n | 平均差 | 標本 SD | 中央値 | 95% CI | exact sign p | 判定 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| BODY_vs_SHUFFLED | 8 | 0.0228199 | 0.0390574 | 0.0270155 | [-0.00387488, 0.0470737] | 0.289062 | FAIL / 未達 |

## 12. BODY vs STALE

実timestampで30秒前のbodyを使い、staleness channelには実際の古さを通知するage-aware対照。欠損はmask0。単なるbody channelの有無と、現在性の寄与を分ける。

| paired 比較 | seed n | 平均差 | 標本 SD | 中央値 | 95% CI | exact sign p | 判定 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| GRU128_BODY_vs_STALE | 8 | 0.0807716 | 0.0714602 | 0.0403134 | [0.0379397, 0.130395] | 0.0078125 | PASS |

新規実ジョブでの補助比較:

| paired 比較 | seed n | 平均差 | 標本 SD | 中央値 | 95% CI | exact sign p | 判定 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| BODY_vs_STALE | 8 | 0.0789245 | 0.0912334 | 0.0682182 | [0.0217792, 0.139832] | 0.289062 | FAIL / 未達 |

## 13. counterfactual body

同じtask・重み・Aの過去履歴・hiddenをcloneし、最終bodyだけA/Bへ分岐する。対応bodyのactionとAの固定actionをBの実測costで採点する。全ての同task・異block ordered pairを対象とし、改善するpairだけを選ばない。

| paired 比較 | seed n | 平均差 | 標本 SD | 中央値 | 95% CI | exact sign p | 判定 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| GRU128 | 8 | 0.0124385 | 0.00269173 | 0.0131545 | [0.0104745, 0.0139326] | 0.0078125 | PASS |

| 構造 | seed | pair数 | 行動変化率 | matched効用 | frozen効用 | 効用差 |
| --- | --- | --- | --- | --- | --- | --- |
| GRU128 | 0 | 3840 | 0.10625 | 0.214809 | 0.200978 | 0.0138315 |
| GRU128 | 1 | 3840 | 0.133854 | 0.21936 | 0.204432 | 0.0149281 |
| GRU128 | 2 | 3840 | 0.0981771 | 0.215755 | 0.202489 | 0.0132663 |
| GRU128 | 3 | 3840 | 0.11901 | 0.215965 | 0.201257 | 0.014708 |
| GRU128 | 4 | 3840 | 0.108073 | 0.215689 | 0.2041 | 0.0115885 |
| GRU128 | 5 | 3840 | 0.0723958 | 0.207821 | 0.201297 | 0.00652361 |
| GRU128 | 6 | 3840 | 0.107292 | 0.220408 | 0.207366 | 0.0130427 |
| GRU128 | 7 | 3840 | 0.102865 | 0.216529 | 0.204909 | 0.0116196 |

モデル内部のbody入力への介入と保存済み実測outcomeの再生であり、物理身体そのものを無作為化した実験ではない。行動変化だけでPASSにしない。

## 14. temporal OOD

sampling interval、更新遅延、jitter、dropout、30秒STALE、task履歴長変更をcontrolled perturbationとして評価。固定step暗記や実時間での頑健性を当然とは扱わない。

| 条件 | seed n | 効用 | 遅延 秒 | 締切内成功率 | 実行失敗率 |
| --- | --- | --- | --- | --- | --- |
| body_update_delay | 8 | 0.222809 | 0.0082419 | 0.754883 | 0 |
| network_jitter | 8 | 0.223456 | 0.00824082 | 0.756836 | 0 |
| sampling_interval_2x | 8 | 0.218646 | 0.00836346 | 0.756836 | 0 |
| stale_frames | 8 | 0.14448 | 0.0119024 | 0.694336 | 0 |
| task_start_timing | 8 | 0.221298 | 0.008161 | 0.764648 | 0 |
| temporal_sensor_dropout | 8 | 0.221056 | 0.0081946 | 0.762695 | 0 |

最悪の保存条件: stale_frames、平均効用 0.14448。大幅低下も省略しない。

## 15. sensor OOD

noise、dropout、constant、scale、GPU sensor欠損、Mac欠損、network latency、inversion、permutationを評価。controlled_resource_unavailableだけはRTX選択時のfailureを合成した資源障害simulationで、他の観測介入と分ける。実GPUを故障・停止させた試験ではない。

| 条件 | seed n | 効用 | 遅延 秒 | 締切内成功率 | 実行失敗率 |
| --- | --- | --- | --- | --- | --- |
| constant_value | 8 | -0.350292 | 0.0874144 | 0.380859 | 0 |
| controlled_resource_unavailable | 8 | -0.919386 | 0.00824975 | 0.03125 | 0.850586 |
| delayed_telemetry | 8 | 0.14448 | 0.0119024 | 0.694336 | 0 |
| mac_unavailable | 8 | 0.224199 | 0.00817206 | 0.763672 | 0 |
| network_latency_increase | 8 | 0.225616 | 0.00834471 | 0.757812 | 0 |
| partial_sensor_inversion | 8 | 0.0203761 | 0.0333226 | 0.618164 | 0 |
| rtx3060_sensor_unavailable | 8 | 0.214955 | 0.00824975 | 0.760742 | 0 |
| scaled_telemetry | 8 | 0.221501 | 0.00841588 | 0.756836 | 0 |
| sensor_dropout | 8 | 0.221056 | 0.0081946 | 0.762695 | 0 |
| sensor_noise | 8 | 0.221533 | 0.00884323 | 0.756836 | 0 |
| sensor_permutation | 8 | 0.130747 | 0.0191241 | 0.691406 | 0 |

最悪の保存条件: controlled_resource_unavailable、平均効用 -0.919386。大幅低下も省略しない。

## 16. safety

背景GPU duty<=50%、256MiB/device未満のmatrix、CPU背景<=2thread、GPU75℃以上または必須観測不能で停止する。OOM storm・thermal shutdown・電源断・host crashを目標にしない。センサー取得だけで安全運用の完了とはしない。

| 最終安全監査 | 値 |
| --- | --- |
| 安全判定 | PASS |
| 既存成果物の保全 | PASS |
| 連続telemetry | PASS |
| 固定frame検証 | PASS |
| runtime cleanup | PASS |

実測温度・負荷・daemon overheadの記述統計は4–5節を参照。全監査値と資源別の詳細は [resource_summary.json](resource_summary.json)、終了時の状態は [final_runtime_state.json](final_runtime_state.json) に保存する。

## 17. statistics

独立単位は training seed 0–7。同seed・同評価task列の平均utility差をpairedで比較し、mean・sample SD・median・20,000回seed bootstrap 95% CI・両側exact sign pを保存する。各primaryは n>=8、mean>0、CI下限>0、p<=.05の積条件。telemetry sampleや同一seedのepisodeを独立nとして増やさない。

CIは固定held-out workloadを共有した学習seed変動だけを表す。未知workload母集団や長期環境変動は含まない。複数比較のpは未補正で、事前指定primaryは全条件成立を要求し、補助比較は探索的に解釈する。

統計機械可読値: `report_statistics.json`、`ablation_results.json`、`live_results.json`。

## 18. limitations

資源別コスト行列は順次実測で同時反実仮想ではない。replayで良好でも実ジョブの有効性は別検証が必要。新規liveも同一装置・有限workload cohortであり、一般的な身体・未知機械・長期日常環境への汎化を証明しない。

低次元の負荷・圧力はproxyを含む。Mac生温度は未取得で、memory proxyとOS pressureは異なる。sensor permutation/noise等は合成介入、実ネットワーク障害・GPU故障・危険温度試験ではない。行列計算jobの効用改善を言語能力や主観的感覚の証拠へ拡張しない。実ジョブのfinite/checksum検査は出力行列の最初の1行を対象とし、全要素の数値正当性検査ではない。独立double参照との検査行checksum差は最大7.76e-7だった。GRU64・PPO・MASTER_ONLY/MAC_ONLYの独立比較は未実施で、その優劣やMac追加価値は未確認。Mac欠損の合成介入だけで末梢ノードの追加価値を証明しない。

無負荷・高負荷の区別だけで成功としない。現在の正しいbodyとdownstream outcomeの改善が成立しないときは研究FAILを維持する。

## 19. success criteria

| 成功条件 | 判定 |
| --- | --- |
| A_telemetry | PASS |
| B_fixed_frame | PASS |
| C_BODY_vs_BLIND | PASS |
| D_BODY_vs_SHUFFLED | FAIL |
| E_BODY_vs_STALE | PASS |
| F_counterfactual | PASS |
| G_live_BODY_vs_BLIND | PASS |
| G_live_BODY_vs_TRAINED_BLIND | FAIL |
| H_safety | PASS |
| I_reproducibility | PASS |
| baseline_preservation | PASS |
| runtime_cleanup | PASS |
| probe_validation_gate | FAIL |
| confirmatory_scope | FAIL |
| evidence_readable | PASS |

全条件の結合: **FAIL**。欠損・読取失敗・未実施は未検証でありPASSへ置換しない。

## 20. Kamimusuhiへの意味

機械身体情報を取得・正規化・入力・検証する実験基盤と、研究上成立した条件を残す。全gateが成立していないため、「身体感覚を獲得した」「実資源判断を有意に改善した」という総合的な成功主張はしない。失敗条件からsensor設計・学習・評価のどこを改善すべきか判断する。

## 21. 次phase判断

現段階で次phaseへの昇格は推奨しない。SHUFFLED差と新規実ジョブの独立BLIND差、probeを新しい独立データと事前固定した設計で再検証することを優先する。今回のtestを使う追加調整は行わない。

K0-G Visual Peripheral Sense、K0-H Auditory Peripheral Senseは次候補として挙げるに留める。camera・microphone・gaze・VAD等を自動実装しない。今回の結果、未達gate、安全・runtime cleanupを確認した後、次phaseへの最終承認を一度だけユーザーへ求める。

## 22. reproducibility

source commit、source file hash、dataset hash、normalization identity、schema、seed、best/final checkpoint hashを保存する。rawは実測を維持し、synthetic介入と別管理する。checkpointバイナリを無条件でGitへ追加しない。接続情報はGit除外済みprivate設定から取得し、報告書・raw・学習configへ記録しない。

| 再現性 | 値 |
| --- | --- |
| source_commit | 0fb0603f3872aed2dbbf1b045769d0dc4dcb37a4 |
| training identities | {"confirmatory_eligible":false,"dataset_sha256":"737441329ab7bd8ea8dd34aa773757f55ff181a82eb635d65079091aff2c725b","decision_sha256":"9c23956d66a5604efe19d17743624adc3745942bce8969d42954ebc5f329d657","experiment_scope":"exploratory_after_failed_probe","exploratory_after_failed_probe":true,"failed_probe_sha256":"633ebbf624e114ea75473a5300f0c925960e0fe5a98aac0b272322897915dae1","normalization_file_sha256":"14bedb0549125626351a7cf8016d0e902484cf105b8d09e04fe6deb74aa66c2a","normalization_identity":"fa4ccb3d405ca670bbfad8b3015fdeb733848a7bf69d308b6df8d617d23065f3","policy_source_sha256":"0365934067b5e5aa81518592e77498fdbd58ade3657e04a7ce7558bfe528270e","raw_sensor_schema_identity":"k0f.raw.v1","research_status_locked":"FAIL","sensor_schema_identity":"k0f.frame.v1","source_files_sha256":{"normalize.py":"4bc3688a37eca612e07eb5feb37ceeac75008da3a739aa365be3b18ec0f8a62f","policy.py":"0365934067b5e5aa81518592e77498fdbd58ade3657e04a7ce7558bfe528270e","probe.py":"75a47f08859468d6ca2e28a2dd3f62b18d1b4ab87d31333e756431ceacd08eef","statistics.py":"4c281a29f50885b4cc668a340c39540211113dd813b4d1906e32499520425141"}} |
| live checkpoint identities | {"0_BLIND":"cdb6e1a1441cd1f5565aa5f133cff89bda67acbba6b52631953c7b24f80e35ee","0_BODY":"4b27aa2fe0aedda1850b18ef300fe2c92ab7d1c269ba593e0179579ff2704c90","1_BLIND":"9cdc9e12f5bc3eb5ba8add1d7b04466c5b3c9d430e340f4224967cf305262748","1_BODY":"23a3e9d093d3348f77854bd408e903af233aef6719fc1261c3fc2140283e9b6c","2_BLIND":"47f9f97bac5f65b5d97edc51324feabbe1a7aa57eb63a8cba821d15e84772e40","2_BODY":"4dd4b2c3ebc41a89e8929bf55f3cc94034f075e4d0d8fee9e9ef425ba3bc3870","3_BLIND":"fbfadeae5c617ca9879bdf2efad0c12c7d311c268b1565d30c291deea39b0b26","3_BODY":"afc67720286f49f156ed5aff9c223dde1ae6bdad28dbb2d0be686492f7712efe","4_BLIND":"e484a36d37e02cdfc4e80f93a41bfafed5c400b17f0ced9bffbd95e0a1315788","4_BODY":"6edff27a0a3904d62d0b3e61e8eb159f1201dcd580a434ec06590d62778ae28f","5_BLIND":"923bf429477b450bd811e8835252f72b924c540992a8e8d4f135b0f942535e2f","5_BODY":"0a6fbf032be913f8bc7fe30f4ccc9335d7f45183d03382dfeaa216c6679ff363","6_BLIND":"c2c6aa23f4bc7113bd19eb035fb5e83fc6ddf2b4655277520507ddb627ec143c","6_BODY":"ee8c64150391eba222189689b5e340c6748ecff4fd9f13404a10844117aca5a6","7_BLIND":"7646106e296d014771acef24382eaeda2d5bf538e518c01c0dc1ae0fbb560fac","7_BODY":"00edb79b88d71ad65f43ba223d096493f3bb115d5a52756b940c63efc5e4e988"} |
| baseline pass | PASS |
| reproducibility_pass | PASS |

保存結果からの再生成:

```sh
python -m experiments.k0_f_interoception.visualize --artifacts ARTIFACTS
python -m experiments.k0_f_interoception.report --artifacts ARTIFACTS --policy-artifacts POLICY --live-artifacts LIVE
```

収集・学習の正確な引数と停止手順は実験READMEを参照。必須図: [body_timeseries.png](body_timeseries.png)、[thermal_load_relationship.png](thermal_load_relationship.png)、[resource_pressure.png](resource_pressure.png)、[network_body_state.png](network_body_state.png)、[prediction_probe.png](prediction_probe.png)、[body_ablation.png](body_ablation.png)、[counterfactual_body.png](counterfactual_body.png)、[ood_heatmap.png](ood_heatmap.png)、[policy_action_by_body_state.png](policy_action_by_body_state.png)、[pareto.png](pareto.png)。

## 23. runtime cleanup

| 最終runtime | 値 |
| --- | --- |
| schema_version | k0-f-final-runtime-v1 |
| recorded_at_utc | 2026-09-10T12:13:26.170886+00:00 |
| cleanup_pass | PASS |
| mac_owned_experiment_pids | [] |
| owned_ui_mirror_sleep_inhibitor | [{"pid":30416,"role":"gui.pid","stopped":true},{"pid":23272,"role":"mirror.pid","stopped":true},{"pid":26621,"role":"finalization_caffeinate.pid","stopped":true}] |
| remote | {"compute_processes":[],"cpu_percent":1.3,"gpu_summary":["NVIDIA GeForce RTX 3060, 35, 0 %, 40 MiB, 12288 MiB, 14.54 W","Tesla P100-PCIE-16GB, 37, 0 %, 4 MiB, 16384 MiB, 24.21 W"],"load_average":[0.11767578125,0.4072265625,0.3984375],"memory_available_bytes":29241044992,"owned_experiment_pids":[],"scratch_files":[],"services":{"k0-e2-j72-eval.service":"inactive","k0-j72-eval.service":"inactive","llama-master.service":"inactive"},"timestamp":1789042349.4306045} |
| services_match_initial | はい |
| mac_final_thermal | nominal |
| mac_cpu_utilization_last | 0.173732 |
| master_temperature_c_last | {"cpu_temperature_c":35.0,"p100_temperature_c":37.0,"rtx3060_temperature_c":35.0} |
| final_sensor_snapshots | three finite samples each; samplers exited normally |
| temporary_services_created | いいえ |
| persistent_power_setting_changed | いいえ |
| training_workers_stopped | はい |
| workload_and_sensor_cleanup_confirmed | はい |
| user_data_or_existing_artifacts_deleted | いいえ |
| push_performed | いいえ |
| owned_artifact_sockets | [] |
| gui_live_action_native_verified | CPU / STALE / hidden 4.453 / frozen-policy new real job provenance visible |

停止したことと負荷・温度の通常範囲への復帰を別々に確認する。今回所有したsensor・training worker・背景workload・transportのみを終了し、既存serviceを開始前状態へ戻す。無関係なGPU processを停止しない。停止証拠がなければcleanupをPASSにしない。

## 24. commit一覧

| commit | 内容 |
| --- | --- |
| f592b49d1ddf63ac15d354bcf351727704079c3e | docs: fix K0-F interoception protocol and preserve prior experiments |
| 3e483dc5acb628fa8c11b93ec26850a706258fcf | feat: add distributed machine telemetry and versioned body frames |
| 43643d7361a0b21b0a2433ece9a9c33510480497 | feat: add bounded real workloads and distributed telemetry acquisition |
| 70b0bf8a4ebc1b84f9c06ed06aea305831a2261b | feat: add paired recurrent body policies and causal information evaluations |
| 28b582151e61389944795160bc1d2c6a1047186c | feat: add Japanese interoception dashboard and research plots |
| eb9b1c2193877b209f7faa1391d5477a86241e95 | fix: preserve matched histories and enforce held-out evaluation gates |
| 6a8a6b70deb324402845f3014ac0704d170fa357 | feat: validate body-selected jobs on live hardware and audit provenance |
| 74f099491378a0fddd036342f83969a5a7d15c30 | fix: retain interrupted recordings and prevent idle sleep during acquisition |
| 9bb0429eb0e047ef01fbbb17ba326979434c7e71 | test: verify acquisition boundaries and add finite dashboard mirroring |
| 846ad92beacbb0484d4b48b0a4139458469986c8 | docs: generate evidence-gated K0-F reports and document live verification |
| 16be1e767bb7c365f11e0e4400fdba46f687b541 | test: reject malformed evidence before declaring research success |
| 89401bd112d7a07d37a22f4e8f5b587c17992867 | feat: expose validation progress and distinguish recorded LLM workloads |
| 7eb4151ede8f018d849fa881627447975ad7c4ea | fix: bound probe scaling of low-variance body measurements |
| 0fb0603f3872aed2dbbf1b045769d0dc4dcb37a4 | feat: isolate exploratory diagnostics after failed informativeness gate |
| f0d9243ddfe03320406bd9a02b18aa384535087b | fix: show failed validation probes and audited diagnostic results clearly |
| e41e42f4ec677f63e28f3713b72638856523afe5 | docs: report failed research gates and concrete verification limits |
