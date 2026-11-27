# 記憶書き込みの空振り検知（memory stall signal）

会話は続いているのに正本の記憶（`state_records`）が増えない状態を、
運用者が既存の経路で読めるようにする信号。実装済みの内容のみを記述する。

## 背景

ライブの対話ターンは正本の記憶へほぼ直書きしない。記憶は主に反省サイクル
（reflector）が提案を出し、Mutation Policy と Continuity Kernel が受理したときに増える。
したがって reflector が壊れる・起動条件が外れる・提案が毎回拒否される、のいずれでも、
会話は普通に続くのに記憶だけが増えない状態になりうる。

別システム FIO には記憶の書き戻しが黙って止まった前歴があり、記憶が増えなかったターンを
理由つきで記録して連続時に警告する仕組みを持つ。本信号はその考え方をかみむすびの
永続状態に合わせて導出したもの。

## 何を入れたか

```text
kamimusuhi-runtime memory-stall --dir <d> [--min-user-utterances N] [--min-quiet-secs S]
   │  Runtime::open（writer epoch なし）→ SqliteStore::memory_activity（SELECT のみ）
   │  → memory_stall::assess（純関数）→ JSON
   ▼
resident memory_stall poller（既定 900 秒ごと・dialogue のある node のみ）
   ├─ GET /status の "memory_stall"
   ├─ kamimusuhi status の MEMORY 行（stalled で終了コード 1）
   └─ logs/memory_stall（判定が変わったときだけ 1 行）
```

- **派生・読み取り専用**。新しいテーブル・migration・正本の書き換えはない。
  集計は既存の `state_records`・`evidence_records`・`mutation_proposals`・
  `mutation_decisions`・`c0_proposals` に対する `SELECT` だけで、1 つの読み取り
  transaction（書かずに破棄）で同じ snapshot から数える。
- **検知して知らせるだけ**。反省の再実行・自動修復・設定の書き換えはしない。
  記憶が再び増えれば次の確認で自然に `ok` に戻る。

### 信号の定義

基準時刻 `baseline` = 個体の最新の `state_records.created_at`
（記憶が一度も無ければ個体の作成時刻。`baseline` は `last_memory` / `individual_created`）。

| 項目 | 定義 |
|---|---|
| `user_utterances_since_memory` | `kind='user_utterance'` かつ `retention_class='standard'` の evidence で `created_at > baseline` の件数（全 source）。判定に使うのはこれだけ |
| `last_user_utterance_at` | 上の standard 発話のうち最新の時刻 |
| `ephemeral_user_utterances_since_memory` | 同期間の `retention_class='ephemeral'` の利用者発話（公開チャンネルの非 operator 発話など）。**参考値で判定には使わない** |
| `quiet_secs` | `(now − baseline) / 1000`（時計が baseline より前なら 0） |
| `reflections_since_memory` | `kind='reflection'` の evidence（反省 1 サイクルに 1 件）で `created_at > baseline` |
| `reflection_drafts_rejected` | 上の反省記録の `drafts_rejected` 配列長の合計（intake 前に反省自身が落とした草稿） |
| `memory_proposals` | `mutation_proposals.recorded_at > baseline` の件数 |
| `memory_decisions` | それらへの `mutation_decisions.disposition` 別件数（accept / reject / quarantine） |
| `c0_proposals` | 同期間の operative lane（C0）提案の status 別件数。正本の記憶は増やさないが「パラメータだけ調整している reflector」と「何もしていない reflector」を区別するために出す |

判定:

```text
stalled ⇔ user_utterances_since_memory ≥ min_user_utterances
          ∧ quiet_secs ≥ min_quiet_secs
```

Ephemeral の発話は設計上記憶へ昇格させにくいので数えない。公開の場で他人がよく話すだけで
停滞と判定される誤警報を避けるため。件数は参考値として報告に残り、運用者は
「公開の場の発話は N 件あったが数えていない」と読める。

両方の閾値は**境界を含む**。`min_user_utterances` は 1 以上に制限するので、
発話の無い個体はどれだけ時間が経っても停滞にならない。

既定値（保守的・両方が必要）: `min_user_utterances = 40`、`min_quiet_secs = 172800`（48 時間）。
一晩に集中して話しただけ、あるいは数件しか話していない静かな 1 週間は停滞にならない。

理由コード（`reason`）:

| コード | state | 意味 |
|---|---|---|
| `NO_USER_UTTERANCES_SINCE_MEMORY` | ok | 最後の記憶以降に利用者発話が無い |
| `BELOW_THRESHOLD` | ok | 発話は積もっているが閾値の片方以上に届かない |
| `STALLED_NO_REFLECTION` | stalled | 反省も記憶提案も 0 件（reflector が走っていない可能性） |
| `STALLED_PROPOSALS_REFUSED` | stalled | reject / quarantine / 反省側の草稿拒否が 1 件以上 |
| `STALLED_NO_MEMORY_PROPOSALS` | stalled | 反省は走ったが記憶提案が 0 件 |
| `STALLED_UNEXPLAINED` | stalled | 提案はあり拒否も無いのに記憶が増えていない |

`STALLED_*` は件数から読んだ診断の手がかりで、原因の証明ではない。

### resident 側の設定

`deploy/resident/pi.resident.example.json` に例を追加した。ブロックを省略しても既定値で有効
（`dialogue` が無い node では起動しない）。止める場合は `"enabled": false`。

```json
{
  "memory_stall": {
    "enabled": true,
    "check_interval_secs": 900,
    "min_user_utterances": 40,
    "min_quiet_secs": 172800,
    "timeout_secs": 45
  }
}
```

範囲: `min_user_utterances` 1–1,000,000、`min_quiet_secs` 0–31,622,400、
`check_interval_secs` 60–86,400、`timeout_secs` 1–600。未知のキーは拒否する。
閾値は C0 パラメータではなく resident 設定に置き、runtime には CLI 引数で渡す
（runtime 側の既定値は同じ定数）。

### 出力経路の選択

既存の運用者向け経路のうち、`/status` と `kamimusuhi status` を選んだ。
反省サイクルの状態（`cognition`）が既にここに出ており、同じ画面で
「反省は動いているか」と「記憶は増えているか」を並べて読めるため。
resident は正本の store を自分で開かず runtime の子プロセスに読ませる、という
`/v1/kamimusuhi/memory` と同じ規則に従う。`kamimusuhi-monitoring` は Prometheus の
usage 指標から Grafana dashboard を生成するもので、状態の判定を載せる経路ではないため使わない。

`kamimusuhi status` の表示例:

```text
MEMORY  [WARN] state=stalled reason=STALLED_NO_REFLECTION utterances_since_memory=55/40 quiet=604800s/172800s last_memory=2026-11-20T00:00:00Z
  since then: reflections=0 memory_proposals=0 refused=0 ephemeral_utterances_not_counted=312 checked=2026-11-27T00:00:00Z
```

- `stalled` は全体の健全性を落とす（終了コード 1）。
- 確認自体が失敗した場合は `state=unavailable` と安定した理由コード
  （`MEMORY_STALL_PROCESS_FAILED` 等）を出し、直前の判定を `last_verdict` に保持する。
  これは停滞とは扱わない（runtime の版ずれ等で全体を赤にしないため）。
- `/status` に載せるのは列挙したフィールド（コード・件数・時刻）だけで、
  会話や記憶の本文、stderr は載せない。

## 実装の所在

| 層 | 場所 | 内容 |
|---|---|---|
| store | `crates/kamimusuhi-store-sqlite/src/activity.rs` | `MemoryActivity`・`SqliteStore::memory_activity`（SELECT のみ・単一読み取り transaction） |
| runtime | `crates/kamimusuhi-runtime/src/memory_stall.rs` | `MemoryStallThresholds`（既定・検証）、純関数 `assess`、`report` / `report_at`、理由コード |
| runtime | `crates/kamimusuhi-runtime/src/main.rs` | `memory-stall` コマンドと `--min-user-utterances` / `--min-quiet-secs`（他コマンドでは usage error） |
| resident | `crates/kamimusuhi-resident/src/memory_stall.rs` | poller・子プロセス実行・フィールドの許可リスト・判定変化時の journal |
| resident | `config.rs` / `state.rs` / `status.rs` / `main.rs` | `MemoryStallConfig`、`Shared::memory_stall`、`/status` への掲載、`MEMORY` 行の描画、起動 |

identity manifest 対象のファイル（`idle_cognition.rs`、`c0/reflection.rs`、`memory_writeback.rs` 等）は変更していない。

## 検証

- runtime unit（`memory_stall.rs` 内 7 件）: 既定値と検証、発話 0 件は無期限でも ok、
  両閾値ちょうどで stalled・発話 1 件不足／1 ミリ秒不足で ok、Ephemeral が何件あっても判定は standard だけで決まる、
  時計の巻き戻り、理由コードの分岐、JSON の安定コード。
- runtime 結合（`tests/memory_stall.rs` 4 件・fake persona と `FakeReflector`、API なし）:
  Ephemeral の発話（store に直接追記）だけが積もっても ok で参考値に件数が出る・
  standard 1 件＋Ephemeral 8 件で閾値 1 は stalled、閾値 2 は ok・`last_user_utterance_at` は
  後から来た Ephemeral ではなく standard 発話の時刻。
  発話の無い個体は ok → 「覚えておいて」で記憶が増えたターンの発話は数えない →
  記憶に残らない 3 発話で閾値 3 ちょうど stalled（`STALLED_NO_REFLECTION`）・閾値 4 では ok →
  quiet 境界（3600 秒ちょうど stalled、3601 秒で ok）→ 読み取りで commit 数が変わらない →
  `FakeReflector` の反省サイクルで記憶が増えると ok に戻る。CLI の JSON 出力・既定閾値・
  フラグの適用範囲・不正閾値の拒否。
- resident unit（`memory_stall.rs` 内 5 件・fixture の runtime script）: 既定値・範囲・未知キー拒否、
  stalled が `/status`・`MEMORY [WARN]`・終了判定へ届き（Ephemeral の参考値も `since then` 行まで通る）、
  journal は判定変化時の 1 行だけ、
  ok は全体判定を落とさない、失敗時の安定コードと `last_verdict` 保持・stderr 非公開、
  無効時は子プロセスを起動しない。
- `deploy/resident/pi.resident.example.json` は既存の `deploy_examples_parse` で読み込み・検証される。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
  `cargo build -p kamimusuhi-runtime --bin kamimusuhi-runtime`、
  `env -u HAI_API_KEY cargo test --workspace --no-fail-fast` を実行（結果は作業報告に記載）。

## 限界・未実装

- 実機の常駐（Pi）・実モデルの reflector では確認していない。既定閾値が実運用の会話量に
  対して妥当かは未測定。
- 判定に数えるのは `retention_class='standard'` の利用者発話だけ。Ephemeral は参考値として
  件数だけ出す。`sensitive` の利用者発話は判定にも参考値にも含めない（現状、利用者発話を
  `sensitive` で保存する経路は無い）。operator が公開チャンネルだけで話す運用では standard
  として数えられるが、他の参加者の発話がどれだけ積もっても停滞にはならない。
- 個体全体の信号のみ。subject 別（相手ごと）の停滞は出さない。ある相手の記憶だけが止まり
  別の相手で増えている場合は検知できない。
- 「以降」は `created_at`（store の時計）で測る。同一ミリ秒の記録は「以前」扱い。
  過去の会話を元の時刻で取り込んだ evidence は、基準より古ければ数えない。
- `reflections_since_memory` は記憶を増やした反省自身を含みうる（反省の記録は草稿の受理後に
  書かれるため）。停滞判定には使わず、理由コードの手がかりにだけ使う。
- 記憶の「増加」は行の作成で数える。訂正（supersede）も新しい行なので増加に含まれる。
- 通知は `/status` と journal のみ。Slack 等への自動送信、Prometheus 指標化、
  dashboard の専用表示は追加していない。
- 確認は `check_interval_secs` ごとの子プロセス起動で、store open 時の migration など
  `memory` コマンドと同じ前提に従う。

## 関連

- [`../idle-cognition.md`](../idle-cognition.md) — 反省サイクルの起動条件と `/status` の `cognition`
- [`./memory-writeback-recovery.md`](./memory-writeback-recovery.md) — ターン単位の書き戻し未完了の検出（別の信号）
- [`./recall-v0.md`](./recall-v0.md)
