# k-sense-mac

公開APIとprivilege不要のセンサーだけを使うSwift daemon。Foundation thermalState、Mach CPU/VM/current RSS、IOKit電源、sysctl swap、getloadavg、peer指定時のみICMPを取得する。生のCPU温度・fan・GPU電力は取得不能として扱い、privileged helperは導入しない。

```sh
mkdir -p .local/bin
swiftc -O experiments/k0_f_interoception/mac/k_sense_mac.swift -o .local/bin/k-sense-mac
.local/bin/k-sense-mac --duration 60 --interval 1 --output .local/raw_mac_smoke.jsonl
```

`--peer-host` は実験driverからprivate configurationで与える。daemonは接続先やファイルパスを出力しない。保存ファイルはappend、stdout既定、有限duration既定60秒。プロセス終了後にサービス・socket・一時ファイルを残さない。daemon CPUは自身+完了した子processのCPU時間、RAMは現在resident bytes。Mac memory_pressureは active+wired+compressor / physical memory のproxyであり、OSのmemory-pressureイベントとは異なる。

SSH transportはこのstdout JSONLを受け、master受信側がreceipt_timestampを追加する。raw source timestampは保持する。欠損はnullとquality=0。正規化時は20値と20maskを保存する。
