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
推論スループットは §3.3 で実測したが、それは素体の能力測定であって role の付与ではない。

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
| backend | ARM64 Cortex-A55 × 8。llama.cpp は `libggml-cpu-android_armv8.2_2.so` を自動選択。GPU backend は未使用・未同定 | 部分実測 |
| supported kernels / dtypes | GGUF Q8_0 の実行を確認。他 dtype は未確認 | 部分実測 |
| usable memory | MemTotal **2,896,204 kB (2.76 GiB)** / MemAvailable **912,684 kB** (整理後・アイドル時) | 実測 |
| transfer cost | adb over Wi-Fi: 155 MB のライブラリ群 **46.7 MB/s** (burst) / 610 MB のモデル **18.6 MB/s** (sustained, 32.7 s) | 実測 |
| startup cost | OS cold boot → mDNS 再出現まで **約 100 秒** | 実測(OS のみ。workload 起動は未測定) |
| throughput vs batch/context | 下記 §3.3。Qwen3-0.6B-Q8_0 で `pp512` 最大 **57.22 t/s** (冷却下) / `tg128` 最大 **4.52 t/s** | 実測 |
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

### 3.3 スループット実測

`llama.cpp` 公式 Android arm64 prebuilt (`b10909`, build `a2878d30d`) を
`/data/local/tmp` へ push して実行。非 root、GPU 非使用、CPU backend のみ。
モデルは `Qwen3-0.6B-Q8_0.gguf` (604.15 MiB / 596.05 M params)。`-r 3`。

| threads | pp512 (t/s) | tg128 (t/s) |
|---:|---:|---:|
| 1 | 12.54 ± 0.10 | 4.01 ± 0.00 |
| 2 | 25.52 ± 0.36 | **4.52 ± 0.03** |
| 4 | 36.68 ± 0.09 | 4.47 ± 0.01 |
| 8 | 50.58 ± 0.17 | 4.26 ± 0.02 |
| 8 (冷却後・単独実行) | **57.22 ± 0.66** | 4.21 ± 0.02 |

**prompt processing と token generation で挙動が完全に分かれる。**

- `pp512` は thread 数にほぼ比例して伸びる (1→8 で **4.03 倍**)。compute bound。
- `tg128` は **まったくスケールしない**。`t=2` の 4.52 t/s が最速で、
  `t=4` 4.47 / `t=8` 4.26 と微減する。memory bandwidth bound であり、コアを足しても改善しない。
  後述の冷却下再測定により、**この非スケールが熱ではなく bandwidth に起因することを確認した**。

観測された peak RSS は **988 MB** (model 604 MiB + KV cache + runtime)。
測定時の MemAvailable は約 1.37 GiB で、この構成は収まった。

#### 比較対象 (同一モデル・同一ツール)

| 機体 | backend | threads | pp512 (t/s) | tg128 (t/s) |
|---|---|---:|---:|---:|
| PRITOM P7 / A523 | CPU (armv8.2) | 8 / 2 | 57.22 (冷却下) | 4.52 |
| Apple M2 Max | CPU only (`-ngl 0`) | 8 | 596.08 ± 20.85 | 109.94 ± 1.23 |
| Apple M2 Max | Metal + BLAS | 8 | 8361.75 ± 23.96 | 278.45 ± 0.25 |

| 比 | pp512 | tg128 |
|---|---:|---:|
| M2 Max (CPU) / P7 | **10.4×** | **24.3×** |
| M2 Max (Metal) / P7 | **146×** | **61.6×** |

CPU 同士でも tg で 24 倍の差がある。**`latency-architecture.md` の観点での実効値**は、
512 token の prompt に対し 128 token を生成する 1 ターンで
prefill 約 **8.9 s** (冷却下) 〜 **10.1 s** (スロットリング下) + decode 約 **28.3 s**
= **約 37〜38 s**。**支配項は decode であり、prefill 側の熱状態は総時間をほとんど動かさない。**

注意: Mac 側は homebrew の `b10621` (`c1d0e7a00`)、端末側は `b10909` (`a2878d30d`) で
**build が一致していない**。厳密な同一条件比較ではない。

#### 持続負荷時の周波数と温度

benchmark の**負荷区間のみ** (終了時刻 00:27:58 を境に切り出し) を 8〜10 秒間隔で 80 サンプル取得。

```text
温度  min 58.6 ℃ / max 70.9 ℃ / avg 65.7 ℃   (アイドル時は 46.8 ℃)
cpu4  1.800 GHz: 33 / 1.680: 5 / 1.584: 9 / 1.488: 1 / 1.344: 6 / 1.200: 26
cpu0  1.416 GHz: 28 / 1.320: 1 / 1.008: 14 / 0.936: 2 / 0.408: 35
```

**負荷末尾で cpu4 は 1.200 GHz に 19 サンプル連続 (約 2.5 分) 張り付き、
その間の温度は 69.8〜70.5 ℃ で頭打ちになった。**

温度の plateau と周波数の固定値クランプが同時に起きているため、
これは test phase 間の DVFS 揺らぎではなく **thermal throttling と判断する**。
DVFS であれば phase に応じて周波数が上下するが、実測は 1.200 GHz の一点に固定されている。

ただし `scaling_governor` と thermal trip point は非 root では読めず
(`/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor` → `Permission denied`)、
**trip point の値そのものは未確認**である。判断は挙動からの推定にとどまる。

`cpu0` が 408 MHz に落ちている 35 サンプルは、`-t 1` / `-t 2` 実行中に
little cluster がアイドル化したものであり、熱制限ではない。

#### この throttling が測定値を汚染している

`llama-bench` は thread 数の小さい順に実行するため、**`t=8` のテストが最後、
すなわち上記の 1.200 GHz クランプ区間と重なる。**

```text
実行順: t=1(pp,tg) -> t=2(pp,tg) -> t=4(pp,tg) -> t=8(pp,tg)
                                                   ~~~~~~~~ 70℃ / 1.2GHz クランプ下
```

したがって `t=8` の値はクロックが絞られた状態で測定されている。
**この交絡を切り分けるため、48.3 ℃ まで冷却してから `t=8` を単独実行で再測定した。**

#### 冷却下再測定による切り分け

| test | 高温連続実行 `t=8` | 冷却後単独実行 `t=8` | 差 |
|---|---:|---:|---:|
| `pp512` | 50.58 | **57.22 ± 0.66** | **+13.1 %** |
| `tg128` | 4.26 | 4.21 ± 0.02 | **-1.2 %** |

**結論が分かれた。**

- `pp512` は冷却で **13.1 % 改善**した。prompt processing は
  **thermal throttling の影響を受けていた**。
- `tg128` は **改善しなかった** (むしろ僅かに低下)。token generation は
  クロックを戻しても速くならない。**熱ではなく memory bandwidth が律速である
  ことが、これで確定した。**

この再測定により §3.3 の交絡は解消した。`tg` の非スケールは thermal の副作用ではない。

再測定時の熱挙動 (負荷区間 27 サンプル、5 秒間隔):

```text
温度  min 48.4 ℃ / max 67.2 ℃ / avg 64.8 ℃
cpu4  1.344 GHz: 20 / 1.488: 4 / 1.584: 1 / 1.800: 2
```

**48.4 ℃ から開始しても約 11 秒で 62 ℃ を超え、以後 1.344 GHz に張り付いた。**
高温連続実行時のクランプ値 1.200 GHz より一段高いだけで、
**この個体には持続負荷に耐える熱的余裕がほぼ無い**。
`t=8` の 57.22 t/s も、完全な非スロットリング値ではなく
「冷却状態から始めた場合の上限」である。

注意: 再測定は開始温度が異なる (48.3 ℃ 対 64.0 ℃) ため、
同一条件での再現ではなく **冷却状態からの上限測定**である。

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
  throughput と transfer cost は §3.3 で実測した。**しかし energy / wall-power delta は依然未測定**であり、
  role 判定の材料は揃っていない。本文書は端末に役割を与えない。
- §3.3 の分離は routing に直接効く。**pp はコアを足せば伸び、tg は伸びない。**
  この body に長い生成を投げるのは、コア数を増やしても無意味である。
  冷却下再測定により、tg の頭打ちは thermal ではなく memory bandwidth と確定した。
  **冷却しても、電力を足しても、この上限は動かない。**
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
- **仮説 H4**: K-Edge の scheduler contract (§8) は、device の能力を単一のスカラーで
  持ってはならない。§3.3 の実測では、同じ device・同じ model で
  **pp が 10.4× 差、tg が 24.3× 差**と、負荷の種類によって M2 Max との比が 2 倍以上変わる。
  「この device は N 倍遅い」という表現は、どちらの負荷を指すか明示しなければ意味を持たない。

## 7. 未実施 / 今後の検証

本文書は素体の実測のみである。以下は実施していない。

| 項目 | 対応する未実施テスト |
|---|---|
| 消費電力 / wall-power delta の実測 | — (role 判定に残る最後の未測定項目) |
| thermal trip point の実値 (非 root で読めない) | — |
| Q4 等の他 dtype、長 context (4K/8K) での throughput | — |
| GPU backend の同定と利用可否 | — |
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

§3.3 の throughput 再現（NDK 不要。公式 prebuilt を使う）:

```bash
gh release download b10909 --repo ggml-org/llama.cpp \
  --pattern 'llama-b10909-bin-android-arm64.tar.gz'
tar xzf llama-b10909-bin-android-arm64.tar.gz

# llama-bench に必要なのは以下のみ (計 155 MB)
#   llama-bench libllama-bench-impl.so libllama-common.so libllama.so
#   libggml.so libggml-base.so libggml-cpu-android_armv8.{0_1,2_1,2_2}.so
$ADB -s <IP>:<PORT> shell 'mkdir -p /data/local/tmp/llama'
$ADB -s <IP>:<PORT> push <上記> /data/local/tmp/llama/
$ADB -s <IP>:<PORT> push Qwen3-0.6B-Q8_0.gguf /data/local/tmp/llama/

$ADB -s <IP>:<PORT> shell 'cd /data/local/tmp/llama && chmod 755 llama-bench && \
  LD_LIBRARY_PATH=. ./llama-bench -m Qwen3-0.6B-Q8_0.gguf -t 1,2,4,8 -p 512 -n 128 -r 3 -o md'
```

`/data/local/tmp` からの ELF 実行は非 root の `shell` uid で可能であることを確認済み。
CPU backend は `libggml-cpu-android_armv8.2_2.so` が自動選択される。

## 9. 確認の限界

- 非 root のため、`/proc/swaps`、ブロックデバイス、`scaling_governor`、電力計測値は取得できていない。
- GPU の型番・利用可能な compute backend を同定していない。§3.3 は CPU backend のみの測定である。
- 推論スループットは測ったが **消費電力を測っていない**ため、
  §5 が求める end-to-end usefulness は未完であり、**本機に cognitive role を与える根拠は本文書には無い。**
- §3.3 の Mac 比較は build 版が一致していない (`b10621` 対 `b10909`)。桁の比較には使えるが厳密ではない。
- throughput は 1 モデル (0.6B Q8_0)・1 context 長でのみ測定した。他の規模へ外挿できない。
- `t=8` の交絡は冷却下再測定で切り分けたが、その再測定も 11 秒で 1.344 GHz に張り付いており、
  **完全な非スロットリング値は取得できていない**。この個体では取得手段が無い。
- 測定は 1 個体・1 回の観測であり、同型機で再現するかは未確認。
- §5 への登録可否は未判断。本文書は登録提案ではなく素体の記録である。