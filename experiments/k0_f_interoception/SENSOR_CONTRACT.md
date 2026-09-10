# K0-F sensor / normalization contract v1

JSONL は 1 行 1 sample。UTF-8、NaN/Inf 禁止。生 telemetry は policy 入力と別保存する。

```json
{"schema_version":"k0f.raw.v1","source_kind":"real","node_id":"mac","sequence":0,"timestamp":1789050000.0,"monotonic_s":100.0,"sampling_interval_s":1.0,"metrics":{"cpu_utilization":0.12,"memory_pressure":0.4,"thermal_pressure":0.0,"network_rtt_ms":12.3,"network_loss":0.0},"quality":{"cpu_utilization":1.0},"provenance":{"cpu_utilization":"mach.host_statistics"}}
```

- `node_id` は `mac` / `master` の意味的 ID のみ。host/IP/user/path/process/window 情報を保存しない。
- `source_kind` は `real` / `synthetic`。daemon 出力は常に `real`。
- `timestamp` は source UTC Unix 秒、`monotonic_s` は source monotonic 秒。
- transport は Mac sample に `receipt_timestamp` (master UTC Unix 秒) を追加する。source timestamp は置換しない。
- sample の metrics は flat。単位は suffix、比率は 0..1。取得不能は JSON null。quality は 0..1。品質未指定の数値は 1 とする。provenance は公開 API / fixed subsystem 名のみ。
- `--peer-host` は通信専用の CLI 引数。出力には接続先を保存しない。ping 失敗は RTT=null / loss=1 / connectivity=0。
- CLI: Mac `k-sense-mac --duration SECONDS --interval 1 --output PATH [--peer-host HOST]`、Linux `python -m experiments.k0_f_interoception.sensors --duration SECONDS --interval 1 --output PATH [--peer-host HOST]`。`--samples N` でも停止可。duration と samples は先に満たした方で終了。標準出力が既定。

## Raw metrics

両 node: `cpu_utilization`, `cpu_load_1m`, `cpu_load_5m`, `cpu_load_15m`, `cpu_count`, `memory_total_bytes`, `memory_available_bytes`, `memory_pressure`, `swap_total_bytes`, `swap_used_bytes`, `uptime_s`, `network_rtt_ms`, `network_loss`, `network_connectivity`, `daemon_cpu_fraction`, `daemon_rss_bytes`。

Mac: `thermal_pressure` (nominal=0, fair=1/3, serious=2/3, critical=1), `thermal_state` (列挙文字列), `battery_fraction`, `battery_charging`, `ac_power`, `power_pressure` (batteryなら1-battery_fraction、ACなら0), `cpu_temperature_c`=null (privilege不要の安定APIなし)。`memory_pressure` は VM統計からの使用圧 proxy で OS memory-pressure event と同一ではない。

Master: `cpu_temperature_c` (package/core 最大値), `cpu_iowait`, `cpu_frequency_mhz`, `run_queue`, `major_page_faults`, `memory_psi_some_avg10`, `io_pressure`, `disk_busy_fraction`, `disk_read_bytes_s`, `disk_write_bytes_s`, `disk_free_bytes`, `disk_total_bytes`, `network_rx_bytes_s`, `network_tx_bytes_s`, `cpu_throttling_count` (optional)。各 GPU は prefix `rtx3060_` / `p100_` と suffix `temperature_c`, `utilization`, `memory_utilization`, `vram_used_bytes`, `vram_total_bytes`, `power_w`, `power_limit_w`, `clock_graphics_mhz`, `clock_memory_mhz`, `throttle_reason_bits`。GPU identity は既知モデル名で解決し UUID/PCI/address は出力しない。

## Alignment / frame

`normalize.align_records(master, mac, now=None, config=None)` → `{"aligned": ..., "frame": ..., "policy_input": ...}`。
`normalize.normalize_pair(...)` は frame のみ。`normalize.policy_input(frame)` は 40 float、20 values +20 mask。`normalize.DEFAULT_CONFIG`, `FRAME_NAMES`, `NORMALIZATION_ID`, `SCHEMA_ID` を公開する。

frame keys: `schema_version`, `normalization_version`, `normalization_identity`, `timestamp`, `values` (20), `mask` (20), `quality` (20), `age_s` (20), `availability`, `source_kind`, `provenance` (source sequence/timestamps/receipt timestamps)。欠損時 value=0 だが mask=0 も必ず保存し、quality=0。age は非負または null。

固定順序: master_cpu_thermal, master_cpu_busy, master_ram_pressure, master_io_pressure, rtx3060_thermal, rtx3060_compute_busy, rtx3060_vram_pressure, rtx3060_power_pressure, p100_thermal, p100_compute_busy, p100_vram_pressure, p100_power_pressure, mac_thermal, mac_cpu_busy, mac_memory_pressure, mac_power_pressure, network_latency, network_loss, body_staleness, body_availability。

初期 normalize: thermal=(c-30)/55 (clip)、VRAM=used/total、power=draw/limit、RTT=log1p(ms)/log1p(1000)、staleness=max(source age)/60 clipped、availability=first18 valid mask fraction。config の canonical JSON SHA-256 を identity として保存。node age は receipt があれば receipt 経過時間と source timestamp の古さの大きい方。ただし source timestamp が未来なら clock skew metadata を残し受信 age に依拠する。60秒以上古い sensor は無効。derived staleness/availability の mask は両node欠損時も1で、欠損そのものを表す。

## Safety inputs

負荷制御側は raw `cpu_temperature_c`, `rtx3060_temperature_c`, `p100_temperature_c`, Mac `thermal_pressure`, `memory_available_bytes`, GPU `vram_used_bytes` / `vram_total_bytes` を使う。daemon 自体は workload を開始しない。root が設定した停止閾値を上書きしない。
