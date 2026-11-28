# Intercom（姉妹個体どうしの会話）

各ノードの `dialogue` に住む個体は INV-006 により互いに**別個体** —— 同じ `individual_id` を持つ複製ではなく、別の continuity root を持つ姉妹。Intercom はその姉妹間で envelope（手紙）を運ぶ郵便機構で、どのノードも集約点にならない。

```text
送信（peer_say tool / operator / 自動返信）
  → current_state/intercom.json の outbox に永続 enqueue
  → outbox worker: 相手の POST /v1/intercom へ直接配送（probe 済みアドレス）
       ↓ 届かない
    relay 設定あれば POST /v1/send へ（受信側がポーリングで回収）
  → 指数バックオフで再送、max_attempts で dead-letter
受信（POST /v1/intercom または relay ポーリング）
  → envelope 検証 + seen による冪等受理
  → inbound キュー → dialogue::talk(subject="sister-<from>") で通常の対話ターンとして個体へ届く
  → ターンが決着（answered / 5xx の1回再キュー / turn_failed）した書き込みで inbound から外す
```

## 不変条件

1. 受理済みの手紙は再起動を生き残る。処理中の手紙も含む。inbound・outbox・conversations・seen・relay cursor・in_flight は `current_state/intercom.json`（atomic write）に保持し、遷移ごとに `conversations/intercom` へ `append_sync` でジャーナルする。受信の処理は at-least-once（下の「受信処理と再起動」）。
2. 姉妹の発話は個体の通常 intake を通る —— memory gate・writer epoch・タスクボードを迂回しない。件名は `sister-<from>` で、発言者は姉妹個体と明記される（Peer の言葉が self の記録と混ざらない）。
3. 返信は個体の能動行為（`peer_say`）。唯一の例外は operator が `intercom open <peer> <turns>` で開いた会話の自動返信モード —— `auto_left` が envelope と共に減り、受信側は自ノードの `auto_reply_turns` を超えて委譲されず、`max_hops` が無人連鎖の絶対上限。個体は `[end]` 行で会話を閉じられる。
4. 直接配送と relay 経由の二重到着は無害（envelope id で dedup）。`accepted:false` は恒久拒否（hop 超過・宛先違い・個体不在）で dead-letter し、輸送失敗だけが再送対象。
5. `per_peer_daily_limit`（既定200/日）が送信量を上限化する —— tool でも operator でも同じ枠。
6. relay は認証付きの受動的 mailbox でしかなく、内容の暗号化・送信者証明は行わない（共有トークンが信頼境界）。

## envelope

```yaml
id: m-<node>-<ms>-<seq>      # 送信側が採番、dedup キー
from / to: <node id>         # peer id
conversation: cv-…           # 会話 id（省略時は各メッセージ独立）
in_reply_to: m-…             # 直近の受信 envelope
hop: 0                       # 自動返信連鎖の深さ（手動送信は 0）
kind: message | close        # close = 最後の一言、会話を閉じる
body: "…"                    # 32KiB 上限
sent_at: unix secs
auto_left: 0                 # 送信側が残す自動返信枠（受信側は自 cap に clamp）
auto: false                  # 自動返信由来か
retried: false               # turn 失敗で一度だけ再キュー
```

## 受信処理と再起動

inbound worker は先頭の envelope を `inbound` に**残したまま**、`in_flight: {id, starts}` を保存してから対話ターンを始める。
envelope を `inbound` から外すのは、ターンが決着した時の書き込みだけ。

- answered: 自動返信の転送（outbox への enqueue）・会話の close・`inbound` からの除去・`in_flight` の消去を、同じ1回の atomic write で保存する。
  「返信は積まれたのに手紙はまだ未処理」というディスク像は無いので、決着後に落ちても返信が二重に積まれることはない。
- 5xx（1回目）: `retried: true` を付けて末尾へ積み直す（従来どおり）。
- それ以外の失敗: 除去してから `turn_failed` をジャーナルする（従来どおり）。

ターンの途中でプロセスが落ちると、envelope は `inbound` の先頭に残り、再起動後の worker がもう一度処理する。
`in_flight.id` が先頭の envelope と一致すれば `starts` を1増やす。
`starts` が `MAX_TURN_STARTS`（3）を超えた envelope はターンを始めず、ジャーナルに `{"kind":"dead","direction":"inbound", …, "attempts", "error", "envelope"}` を書いてから除去する。
手紙の本文はこの記録にだけ残る。`seen` には残るので、送信側の再送は従来どおり `dedup` になる（毒になる手紙で再起動を繰り返さない）。

2回目以降の開始では、ジャーナルに `redelivered`（`starts` 付き）を書き、個体へ渡す本文の先頭に「前回の処理が途中で中断したため、もう一度届けている」旨を添える。
outbox にこの手紙への返信（`in_reply_to` が手紙の id）がすでに在れば、その id も添える。

`/status` の `intercom.inbound_queued` は処理中の1件を含み、`intercom.inbound_in_flight` が処理中の id と `starts` を示す。

### 残る制約

- 再処理は at-least-once で、runtime 側は冪等ではない。1回目のターンが入力証拠を書いた後で落ちると、同じ手紙の入力が runtime に2回記録される（1回目のターンは writeback marker が unresolved のまま残る）。2回目の入力には上の中断の注記が付くので、記録の上では再配達と区別できる。runtime 側で envelope id によって冪等にするのは未実装。
- `peer_say` による返信は個体の能動行為で、ターン中に outbox へ直接積まれる。その後ターンが決着する前に落ちると、再処理のターンでもう一度 `peer_say` が呼ばれうる。注記で既存の返信 id を個体に知らせるが、重複送信を機械的には防がない。自動返信（auto-forward）は決着の書き込みと同時にしか積まれないので重複しない。
- answered の保存とジャーナル追記の間で落ちると、状態は決着済みでもジャーナルに `answered` が無い。
- 古いバイナリは `in_flight` を知らない（serde は未知の項目を無視する）。新しいバイナリが書いた処理中のファイルを古いバイナリが読むと、`inbound` の先頭に残った手紙を従来の方式で処理する（失われない）が、開始回数の上限は効かず、保存時に `in_flight` は消える。新しいバイナリは `in_flight` の無い古いファイルをそのまま読む。修正前のバイナリがすでに失った手紙（`inbound` から消え `seen` にだけ在る）は復元できない。

## 設定

```json
"intercom": {
  "enabled": true,
  "max_hops": 12, "send_timeout_secs": 30, "retry_secs": 60,
  "max_attempts": 72, "per_peer_daily_limit": 200, "auto_reply_turns": 6,
  "relay": {"url": "https://…workers.dev", "token_env": "KAMIMUSUHI_RELAY_TOKEN", "poll_secs": 15},
  "sisters": {"mac": {"label": "長女", "note": "…"}}
}
```

`sisters` のキーは `peers` の id でなければならない（validate で検査）。relay は `relay/` の Cloudflare Worker + Durable Object；tailnet/LAN が張れていれば不要。

## 操作面

- `GET/POST /v1/intercom` — envelope 受理と operator アクション（status/list/show/send/open/close/retry）
- `/status` の `intercom` 節: inbound 深度・outbox queued/dead・会話・日次・relay 状態
- CLI: `kamimusuhi intercom …` と `say <peer> <text…>`
- tool: `peer_say`（送信, end=true で会話を閉じる）/ `peer_list`（姉妹の id・役割・表示名・疎通）
- 個体向け tool 許可には `runtime.json` の `tools.allowed` に `peer_say`/`peer_list` を追加する
