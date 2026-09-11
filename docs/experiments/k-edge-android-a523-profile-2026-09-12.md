# 実機プロファイル: Android/ARM64 廉価タブレットを K-Edge 候補として実測する

実測した内容のみを記述する。設計仮説と実測を混同しない。
未測定の項目は「未測定」と明記し、推定値で埋めない。

測定日: 2026-09-12 / 測定手段: LAN 経由 adb (非 root, `ro.build.type=user`)

## 1. 目的

`heterogeneous-cognitive-compute-substrate.md` §5 は Edge デバイスについてこう要求する。

> Treat each device as an organ candidate with **explicit capability discovery** rather than
> pretending that all devices form one symmetric pool.

しかし同文書 §6 が具体的な実験対象として挙げるのは Nukui Extreme Node
(TB250-BTC PRO + RX 5500 XT / RX 6400 / GTX 550 Ti / GT710-730 クラス) のみであり、
**K-Edge クラスの実測済みプロファイルが 1 件も存在しない**。

`organ-contracts-and-implementation-plan.md` §6.2 は次を禁じている。

> 具体的な力・速度・熱の上限は body ごとに検証し、**共通の架空値で保証しない**。

本文書は、実在する 1 台の廉価 Android タブレットについて §5 の capability discovery 項目を
可能な範囲で実測し、K-Edge 側の最初の非架空 body を記録する。

**この端末を Kamimusuhi のノードとして稼働させたわけではない。** 本文書は素体の実測であり、
organ 実装・event bus 接続・稼働実績は一切含まない。

## 2. 対象個体

| 項目 | 値 |
|---|---|
| 機種 | PRITOM P7 (`P7_A05`) |
| SoC | Allwinner A523 / `sun55iw3p1` (board: `saturn` / `exdroid`) |
| CPU | Cortex-A55 × 8。`cpu0-3` max 1.416 GHz / `cpu4-7` max 1.800 GHz |
| OS | Android 14, `UP1A.231105.001.A1`, build 20240507, security patch 2024-04-05 |
| fingerprint | `PRITOM/P7_A05/P7_A05:14/UP1A.231105.001.A1/20240507-183422:user/release-keys` |
| ABI | `arm64-v8a, armeabi-v7a, armeabi` |
| Treble | `ro.treble.enabled=true`, VNDK 33, `first_api_level=34` |
| パーティション | A/B (`slot_suffix=_a`) + Virtual A/B + dynamic partitions (super) |
| ブートローダー | **locked** (`verifiedbootstate=green`, `flash.locked=1`, `veritymode=enforcing`) |

**全コアが A55 である。big.LITTLE ですらなく、性能コアが存在しない。**

## 3. §5 capability discovery 項目の実測結果

§5 が列挙する項目に対応させる。**半数は非 root では測定できない。**

| §5 の項目 | 実測値 | 測定状況 |
|---|---|---|
| backend | ARM64 Cortex-A55 × 8 / Mali 系 GPU (詳細未同定) | 部分実測 |
| supported kernels / dtypes | — | **未測定** |
| usable memory | MemTotal **2,896,204 kB (2.76 GiB)** / MemAvailable **912,684 kB** (整理後・アイドル時) | 実測 |
| transfer cost | — | **未測定** |
| startup cost | OS cold boot → mDNS 再出現まで **約 100 秒** | 実測(OS のみ。workload 起動は未測定) |
| throughput vs batch/context | — | **未測定。推論 benchmark を一切実行していない** |
| energy / wall-power delta | — | **未測定** |
| reliability / thermal | 下記 §4 | 実測 |

### 3.1 熱

| 状態 | `cpul_thermal_zone` | `gpu_thermal_zone` |
|---|---|---|
| 高負荷 (起動直後の一斉更新) | **66.2 ℃** | 61.4 ℃ |
| 起動直後 | 64.0 ℃ | 58.8 ℃ |
| アイドル | 46.8 ℃ | 46.0 ℃ |

HAL が公開する cooling device は `cpufreq-cpu0` のみ。
**持続負荷時の throttling 曲線は未測定。**

### 3.2 感覚器 (`sensory-nervous-system.md` 側の前提として)

| 受容器 | 有無 |
|---|---|
| 加速度計 | **あり**（Mi3da / miramems, 1-100 Hz, no batching, non-wakeUp） |
| ジャイロ / 地磁気 / 照度 / 近接 | **すべて無し** |
| GPS | **無し**（`location.network` のみ。`location.gps` feature 不在） |
| カメラ | 前面 + 背面 + flash |
| マイク | あり |
| 画面 | 600 × 1024, 160 dpi (実 177.2 × 167.8 dpi), **57.0 Hz**, 最大輝度 500 nit |
| 拡張 | **`usb.host` (OTG) あり** / `ethernet` あり / BLE / Wi-Fi Direct / MIDI |
| モデム | **無し**（`pm list features` に `telephony` 系が 0 件） |

センサー融合は全て無効 (`9-axis fusion disabled`, `geomag fusion (no gyro) disabled`)。
**この body は「自分の姿勢を 1 軸の粗い加速度計でしか知らない」個体である。**

## 4. 主要な発見: swap が存在しない (ROM のビルド不良)

カタログ上は RAM 拡張機能を持つとされるが、**実機に swap は存在しない。**

```text
SwapTotal:             0 kB
/sys/block/zram0/disksize = 0
settings global zram_enabled = 1     ← framework 側の設定は有効
```

原因は firmware のビルド不良であり、設定で有効化できるものではない。**2 箇所で独立に失敗している。**

**(a) fstab 断片が本体へ合体されていない**

Android init は `/vendor/etc/fstab.${ro.hardware}` のみを読む。この機体は
`ro.hardware=sun55iw3p1` なので `fstab.sun55iw3p1` が唯一の対象だが、
**そのファイルに swap 行が 1 行も無い**（全 22 行 / 2877 bytes を確認）。

一方 `/vendor/etc/` には zram 行を持つ断片が 7 本、未使用のまま残っている。

```text
fstab.sun55iw3p1   2877 bytes   22行   ← 実際に読まれる。swap行なし
fstab.wswap         101 bytes    1行   zramsize=90%,zram_backingdev_size=256M
fstab.nswap          86 bytes    1行   zramsize=75%
fstab.256m/512m/1024m/2048m/4096m   101-102 bytes each
```

サイズから明らかなように、これらは代替 fstab ではなく **1 行の断片**である。
BSP のビルド時に本体へ追記される前提の構成が、追記されないまま出荷されている。

**(b) `swapon_all` の呼び出しが存在しない**

`/system/etc/init/hw/init.rc` (58,375 bytes / 1,158 行、読み取り成功) を検索しても
**`swapon` という文字列が 1 件も存在しない**。
仮に fstab を修正しても、それを読む処理が無い。

### 4.1 この個体で観測された実際の退化

初回接続時 (工場出荷状態からの初期セットアップ中、uptime 7 分) の実測値。

```text
load average: 21.46, 21.89, 10.38      (8 コアに対し約 2.7 倍)
MemFree:      32,396 kB                 MemAvailable: 709,912 kB
CPU:          443% nice / 29% idle
  dex2oat32                    355%    (Play Store の AOT コンパイル)
  kswapd0                       37%    ← swap 不在のままページ回収が空転
  com.google.android.gms        77%
```

**swap を持たない 2.76 GiB の K-Edge ノードが usable memory を使い切ると、
`kswapd0` が CPU を焼き続けたまま前進しなくなる。** これは架空値では得られない実測である。

### 4.2 有効化可否

3 経路すべてが塞がっている。

| 方法 | 必要権限 | 可否 |
|---|---|---|
| `/sys/block/zram0/disksize` への書き込み | root | 不可 |
| `swapon` | root | 不可 (`/proc/swaps` すら読めない) |
| fstab 修正 | root + verity 無効化 | 不可 (`/vendor` は erofs ro + `avb=vbmeta`, enforcing) |

ブートローダーは locked。かつ **当該機種の純正 firmware は公開されておらず、
非 root ではブロックデバイスを読めないため full backup も取得できない**
(`dd if=/dev/block/by-name/boot_a` → `Permission denied`)。
したがって unlock は復旧不能のリスクを伴う。**この制約は当面固定として扱う。**

## 5. 参考: 素体整理で回復した範囲

工場出荷状態には、製品 ROM に残置された工場試験アプリが `PERSISTENT` 指定で常駐していた。

| パッケージ | 実体 | 常駐 RSS |
|---|---|---|
| `com.clock.pt1.keeptesting` | `/system/app/KeepTesting` | 56 MB |
| `com.dajingtech.update` | `/product/app/FotaUpdate` (第三者 FOTA, FCM 常時接続) | 63 MB |
| `com.softwinner.runin` | Allwinner run-in test | 59 MB |
| `com.example.test1` | `/system/app/laohua` (老化試験)。**DEBUGGABLE フラグ付き** | — |

`PERSISTENT` 指定は lowmemorykiller の対象外であり、`am force-stop` も効かない
(uninstall 後も再起動までプロセスが残存することを実測)。

計 45 パッケージを `pm uninstall -k --user 0` で user 0 から除去 (211 → 166)。
結果、アイドル時 `load average 0.44` / `MemAvailable 912,684 kB`。
**すべて `cmd package install-existing` で復元可能であり、パーティションは変更していない。**

## 6. 解釈と設計仮説 (実測ではない)

ここから下は **our interpretation / design hypothesis** であり、検証されていない。

### 6.1 解釈

- この個体は §5 が想定する「非対称な organ candidate」の典型例である。
  全コア A55・swap 不在・センサー 1 種という構成は、GPU farm 側の前提と共有できる部分が少ない。
- §5 の結論「A card earns a cognitive role from **measured end-to-end usefulness**」に従うなら、
  **本文書は端末に役割を与えていない。** throughput / energy / transfer cost が未測定である以上、
  cognitive role の判定材料は揃っていない。
- 一方で reliability / thermal / usable memory / 感覚器 は実測できた。
  これらは「何をさせられないか」を先に確定させる材料になる。

### 6.2 仮説

- **仮説 H1**: K-Edge の budget 設計を「usable memory」単独で行うと、この個体では破綻する。
  §4.1 の実測は、残メモリが線形に減るのではなく、ある点から `kswapd0` に CPU を奪われて
  **計算能力ごと落ちる**ことを示す。`Cognitive Budget` (§7.1) は memory と CPU を
  独立変数として扱えない body が実在する、という前提を持つ必要がある。
- **仮説 H2**: `swapon_all` 不在のような **firmware 側の契約違反**は、
  capability discovery が「設定値」ではなく「実測」を見なければ検出できない。
  この個体は `zram_enabled=1` と報告しながら swap を持たない。
  **デバイスの自己申告を信用してはならない実例。**
- **仮説 H3**: 加速度計 1 種・GPS 無し・clock 校正不明というこの body は、
  T05 (時刻誤差の大きい音声・映像、校正失効) の自然な fixture になりうる。

## 7. 未実施 / 今後の検証

本文書は素体の実測のみである。以下は実施していない。

| 項目 | 対応する未実施テスト |
|---|---|
| 推論 throughput / tokens-per-second の実測 | — (§5 の routing 判断に必須) |
| 消費電力・transfer cost の実測 | — |
| 持続負荷時の thermal throttling 曲線 | — |
| usable memory 枯渇時の safe mode 挙動 | **T12** (GPU 不在、stale telemetry → 欠測を healthy としない) |
| 有限 queue / drop・coalesce の実挙動 | **T14** |
| 2 ノード同時所有と旧 owner の fencing | **T20** (G4。物理 2 台目が必要) |
| partition / 移住 / silent fork なし | **T26** (G4) |

**G1 (T01/T02/T03/T06/T07/T15/T25) は単一ノードの契約テストであり、本機を必要としない。**
`organ-contracts-and-implementation-plan.md` の
「G1 を学習済みの novllm 派生 Core 待ちにしない。fake/generic Core で契約の誤りを先に発見し」
という方針と同様に、G1 を実機待ちにする理由はない。

本機が必須になるのは **G4 の T20 / T26**（旧ノードを動かしたままの故障試験）である。
T12 の「GPU 不在ノード」としては G2 段階で投入しうる。

## 8. 測定の再現手順

非 root, LAN 経由。ポート番号は端末再起動のたびに変わる。

```bash
ADB=~/Library/Android/sdk/platform-tools/adb
$ADB mdns services                      # _adb-tls-connect._tcp の IP:port を得る
$ADB connect <IP>:<PORT>

$ADB -s <IP>:<PORT> shell 'grep -iE "swap" /proc/meminfo; cat /sys/block/zram0/disksize'
$ADB -s <IP>:<PORT> shell 'cat /vendor/etc/fstab.$(getprop ro.hardware)'
$ADB -s <IP>:<PORT> shell 'grep -c swapon /system/etc/init/hw/init.rc'
$ADB -s <IP>:<PORT> shell 'dumpsys thermalservice | grep mValue'
$ADB -s <IP>:<PORT> shell 'dumpsys sensorservice | sed -n "/Sensor List/,/^$/p"'
$ADB -s <IP>:<PORT> shell 'pm list features'
```

## 9. 確認の限界

- 非 root のため、`/proc/swaps`、ブロックデバイス、電力計測値は取得できていない。
- GPU の型番・対応 dtype・利用可能な compute backend を同定していない。
- 推論性能を一切測っていないため、**本機に cognitive role を与える根拠は本文書には無い。**
- 測定は 1 個体・1 回の観測であり、同型機で再現するかは未確認。
- §5 への登録可否は未判断。本文書は登録提案ではなく素体の記録である。
