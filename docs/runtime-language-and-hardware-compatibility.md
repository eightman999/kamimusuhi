# 実装言語・実行層・ハードウェア後方互換設計

Status: **living engineering design note / current preferred design**

本書は、[`native-runtime-stratification.md`](./native-runtime-stratification.md) を補完し、Kamimusuhi の実装言語、インタプリタ／動的実行層、CPU ISA・GPU・OS をまたぐハードウェア互換性の方針を具体化する。

現時点の設計結論は次の通り。

> **Rust を常駐する生命維持・神経系 runtime の既定言語、Python を研究・学習言語、C/C++/CUDA/Metal 等を高速化器官、WASM/スクリプト/DSL を交換可能な行動層として扱う。**

Kamimusuhi 全体を単一のインタプリタ上に載せることは既定方針としない。一方で、自己改善・技能追加・実験を高速に回すための動的実行層は明示的に許容する。

---

## 1. 基本原則

Kamimusuhi は単一言語アプリケーションではなく、**native nervous system を中心にした polyglot organism** として実装する。

```text
Kamimusuhi
│
├─ Rust: always-resident organism runtime
│  ├─ heartbeat / lifecycle
│  ├─ event bus / interrupt / cancellation
│  ├─ continuity / state transition
│  ├─ memory plumbing / IPC
│  ├─ sensor and actuator routing
│  ├─ model / organ supervision
│  └─ capability discovery / watchdog
│
├─ Python: research and learning plane
│  ├─ training / fine-tuning
│  ├─ novllm / tokenizer experiments
│  ├─ evaluation / benchmark analysis
│  ├─ memory and cognition prototypes
│  └─ offline consolidation experiments
│
├─ C / C++ / accelerator code: performance leaves
│  ├─ llama.cpp and native inference backends
│  ├─ SIMD / DSP kernels
│  ├─ CUDA / ROCm
│  └─ Metal / platform libraries
│
└─ WASM / script / DSL: mutable behavior layer
   ├─ skills
   ├─ habits
   ├─ experimental policies
   └─ bounded self-modifiable modules
```

言語の役割は固定的な宗教ではない。実測で境界を変更してよいが、**生命維持系を特定の高水準インタプリタ環境の正常性だけに依存させない**ことを優先する。

---

## 2. 「Kamimusuhi はインタプリタか？」

### 2.1 Core はコンパイル済み native binary を既定とする

`kamimusuhi-core` / `kamimusuhi-edge` の最低限の常駐機能は Rust の native binary とする方向を優先する。

Core が担うものの例:

- 起動・停止・再起動・watchdog;
- event loop;
- interrupt / cancellation;
- canonical state mutation boundary;
- continuity / lineage primitive;
- resource-health observation;
- organ discovery;
- local IPC;
- crash-safe journal 境界。

これにより、Python 環境、仮想環境、package dependency、JIT、個別 skill の破損があっても、最低限の runtime を存続させやすくする。

### 2.2 動的に書き換える層は残す

「Core を native にする」と「動的実行を禁止する」は同義ではない。

Kamimusuhi の技能、習慣、実験的 policy、短命な認知 module は、次のいずれかを利用してよい。

- WASM execution substrate;
- Python worker;
- Lua 等の小型 script runtime;
- 将来の Kamimusuhi-specific DSL;
- versioned external executable / plugin。

特に自己改善を扱う場合、毎回 Core 自身を書き換えて再コンパイルするより、**固定された最小実行核 + 権限制限された可変層**の方が rollback、provenance、検証、隔離を行いやすい。

WASM は必ずしも「インタプリタ」に限定されず、実装により interpreter/JIT/AOT を取り得るため、本設計では **sandboxable execution substrate** として扱う。

---

## 3. ハードウェア後方互換の基本方針

Rust を採用しても、バイナリが自動的に古い CPU と互換になるわけではない。互換性は **target triple、CPU baseline、enabled ISA、libc/OS、依存ライブラリ**の組み合わせで決まる。

したがって、配布用 Core と machine-local optimized build を分離する。

### 3.1 配布用 Core は保守的な CPU baseline

配布用 `kamimusuhi-core` は以下を SHOULD とする。

- distributable build で `target-cpu=native` を使わない;
- x86_64 では保守的な baseline ISA を明示する;
- AVX / AVX2 / AVX-512 を Core 起動の必須条件にしない;
- ARM64 では baseline と NEON/platform acceleration の境界を明示する;
- essential path には scalar/reference fallback を残す;
- dependency が暗黙に高い ISA を要求していないことも CI/実機で検証する。

x86_64 の具体的な最低 baseline は、実機テストと配布対象 OS の要件を踏まえて確定する。`x86-64-v1` 相当を一つの候補とするが、名称だけで互換性を保証したことにはしない。

### 3.2 `target-cpu=native` はローカル最適化 build に限定

特定個体・特定 host 向けのローカル build では `target-cpu=native`、PGO、LTO、platform-specific optimization を利用してよい。

ただし、その binary を別世代 CPU へ無条件にコピーしてはならない。配布 artifact とは別 identity/version を持たせる。

---

## 4. ISA / GPU / accelerator は「器官」として分離する

CPU/GPU 固有最適化は Core に埋め込んで起動条件にするのではなく、可能な限り optional accelerator として分離する。

```text
kamimusuhi-core-baseline
        │
        ├─ detect x86/ARM features
        ├─ detect GPU / driver / runtime
        ├─ detect memory / storage / network
        │
        └─ load compatible organs
             ├─ x86-avx2
             ├─ x86-avx512
             ├─ arm-neon
             ├─ cuda
             ├─ rocm
             ├─ metal
             └─ scalar/reference
```

### 互換性 invariant

1. **GPU が無くても continuity/event runtime は起動できる SHOULD。**
2. **AVX2/AVX-512 が無いことを Core 起動失敗の理由にしない SHOULD。**
3. hardware-specific organ は feature detection 後にのみ activate する。
4. unsupported organ の不在は graceful degradation として扱う。
5. canonical state schema と identity lineage は CPU ISA / GPU vendor から独立させる。
6. host migration や accelerator replacement だけを理由に新しい identity を生成しない。
7. 必須機能が accelerator にしか存在しない場合は、その制約を capability manifest へ明示する。

---

## 5. 起動時 capability discovery

Core は「この machine が何者か」を prose ではなく機械的に観測する。

概念的な起動手順:

```text
boot baseline core
      ↓
probe architecture / ISA
      ↓
probe RAM / storage / network
      ↓
probe GPU / accelerator / driver
      ↓
validate organ compatibility
      ↓
activate compatible organs
      ↓
publish Embodiment State / Capability Manifest
```

Capability Manifest の候補:

```yaml
host:
  arch: x86_64
  os: linux
cpu:
  baseline: x86_64
  features: [sse2, avx, avx2]
accelerators:
  - kind: cuda
    device: "..."
    runtime: "..."
organs:
  available: [...]
  disabled_incompatible: [...]
limits:
  memory_bytes: ...
  storage_bytes: ...
```

この情報は Persona Core が自己紹介文として推測して保持するのではなく、runtime owner から `Embodiment State` として参照する。

---

## 6. OS / libc / driver 互換性

CPU ISA だけを揃えても古い machine で動くとは限らない。

特に Linux では次を別々に検証する。

- kernel minimum;
- glibc minimum;
- `musl` static build を利用できる component;
- OpenSSL/TLS 等の native dependency;
- GPU driver / CUDA / ROCm runtime;
- filesystem / syscall assumptions。

可能な component では static/self-contained artifact を検討するが、すべてを無理に static link することは目的にしない。GPU backend は driver/runtime の制約が強いため、Core compatibility と accelerator compatibility を別 matrix にする。

---

## 7. 低性能・旧世代 host の位置づけ

Kamimusuhi は「すべての node で同じモデルを動かす」ことを要求しない。

古い machine は、能力に応じて次の器官を担当できる。

- event relay;
- sensor ingestion;
- watchdog;
- memory cache;
- low-cost reflex;
- storage;
- telemetry;
- offline batch job;
- network gateway。

一方で、重い Persona Core / K-Core inference は適切な GPU / accelerator host に委譲してよい。

したがって、**古い hardware で完全性能を保証するのではなく、古い hardware でも「個体の一部として参加できる」ことを互換性目標とする。**

---

## 8. Build / artifact policy

概念上、artifact は少なくとも次の class に分ける。

```text
release/
  kamimusuhi-core-linux-x86_64-baseline
  kamimusuhi-core-linux-aarch64-baseline
  kamimusuhi-core-macos-aarch64-baseline

accelerators/
  organ-x86-avx2
  organ-x86-avx512
  organ-cuda-<compat-class>
  organ-rocm-<compat-class>
  organ-metal-<compat-class>

local-optimized/
  kamimusuhi-core-<host-id>-native
```

実際の packaging は後で変更してよい。重要なのは **baseline core と hardware-specific optimization の意味を混ぜない**ことである。

---

## 9. Self-modification / plugin safety

可変 script/WASM/plugin 層は、Core と同じ authority を暗黙に持ってはならない。

最低限、次を検討する。

- capability-based permissions;
- CPU / memory / wall-time quota;
- filesystem/network scope;
- canonical-state mutation API の限定;
- version / provenance;
- signature/hash;
- rollback;
- crash isolation;
- deterministic/reference tests where practical。

「自分でコードを書ける」ことと「そのコードが自分の canonical state を自由に書き換えられる」ことは分離する。

---

## 10. 検証 matrix

言語選択と後方互換は宣言ではなく benchmark / compatibility test で固定する。

最低限の検証候補:

```yaml
boot:
  baseline_cpu_only
  unsupported_accelerator_present
  accelerator_missing
  old_linux_userspace
  arm64_edge_host

isa:
  baseline_x86_64
  avx2_enabled
  avx512_enabled
  arm64_neon

failure:
  python_worker_broken
  wasm_skill_trap
  gpu_driver_unavailable
  optimized_organ_illegal_instruction

continuity:
  state_migration_between_hosts
  restart_on_lower_capability_host
  accelerator_replacement
```

期待する性質:

- optional organ failure が Core を巻き込まない;
- capability downgrade が明示される;
- state migration 後も lineage が継続する;
- optimized code の失敗時に baseline path へ戻れる;
- Python/WASM 層の破損だけで heartbeat/continuity が停止しない。

---

## 11. 現時点の採用方針

初期の serious implementation では次を既定とする。

1. **Rust を Kamimusuhi の always-resident Core / nervous-system runtime の第一候補とする。**
2. **Core 自体は interpreter-hosted application を前提にしない。**
3. **Python は training・research・prototype・offline cognition に残す。**
4. **C/C++/CUDA/Metal 等は inference/backend/leaf acceleration に使う。**
5. **WASM/スクリプト/DSL は bounded mutable behavior layer として許容する。**
6. **配布 Core は conservative baseline build、host 固有高速化は別 artifact とする。**
7. **AVX2/AVX-512/GPU は optional organ とし、可能な限り Core boot requirement にしない。**
8. **hardware replacement / migration と identity continuity を分離する。**
9. **最終的な CPU/OS minimum は実機 compatibility matrix と benchmark で確定する。**

一言で表すなら、Kamimusuhi の実装方針は次である。

> **Rust を母語、Python を研究言語、native accelerator を筋肉、WASM/DSL を書き換え可能な技能層にする。**
