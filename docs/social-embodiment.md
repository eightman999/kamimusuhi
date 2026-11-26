# Kamimusuhi Social Embodiment — 基本仕様

**Status:** SPECIFIED  
**Version:** 0.1  
**Target:** Discord / Kamimusuhi Resident

## 1. 基本原則

KamimusuhiはDiscord上に第二の人格を生成しない。既存のContinuity Kernelが保持する同一個体の、社会的な入出力器官としてDiscord Gatewayを扱う。

Discordでの活動は以下の閉ループを満たす。

`Observe → Reason → Propose → Authorize → Act → Verify → Persist`

推論モデルの生成文は、外部へ送信されるまで発話として扱わない。送信の成功が検証されるまで実行済みの事実として記憶しない。

## 2. 社会的認知

- **Reactive Cognition:** 他者から呼びかけられた際に応答する。
- **Background Cognition:** 外部入力がなくても、内部状態や目標を監視する。
- **Initiative Engine:** 研究の進捗や自己の目標を理由に、自発的な行動を提案する。
- **Social Memory:** 会話文脈、自己史、公開知識を区別して想起する。
- **Social Policy Gate:** 発話先、権限、頻度、公開情報を検査する。

## 3. 時間的連続性

プロセス再起動、LLMの切り替え、Discord Gatewayの切断によって個体の同一性を失ってはならない。

進行中の目標・未送信の発話候補・実行結果は永続状態として管理する。

無意識系の監督処理はLLMの応答停止から独立して動く。

## 4. 記憶と証拠

自己の思考、観測、発話、外部で確認された事実、長期記憶を異なる型として保持する。

FIO型の二段想起を採用するが、Discord上の他者の発言を無制限に長期蓄積しない。データ保存は機能に必要な範囲に限定し、公開範囲と削除要求を尊重する。

## 5. 自発的な社会活動

自発的発言には理由となる内部イベント、目的、または検証可能な成果が必要である。

発話候補はSocial Policy Gateを経て、許可されたDiscordチャンネルへ送信する。

単なる活動量の水増し、未承諾DM、自動的な人間関係分析は行わない。

## 6. 完了条件

同一個体が、自らの目標や環境変化から行動の必要性を判断し、許可された範囲で発話し、その結果を検証して自己史に反映できること。

さらに再起動後の継続、権限分離、障害復旧を実環境で検証した時点で、社会的活動基盤として完成と判定する。

---

## 7. 実装契約と境界

### 7.1 閉ループと二段コミット（`Propose → Authorize → Act → Verify → Persist`）

- **Observe / Reason / Propose (`kamimusuhi-runtime`):**
  `DialogueSession::turn_with_social` に `defer_delivery: true` を指定すると、入力観測（応答時は `EvidenceKind::UserUtterance`、自発行動時は `EvidenceKind::SystemEvent` `"social_initiative_observed"`）と生成イベント（`EvidenceKind::SystemEvent` `"response_generated"`）までを記録し、発話候補を `<runtime_dir>/social-proposals/<turn_id>.json` へ原子的に永続化する。この時点では `EvidenceKind::AgentUtterance`、C0 mutation draft 提出、`state_record_usage` の使用回数更新、`memory_writeback_verified` は一切実行しない。
- **Authorize (`kamimusuhi-resident`):**
  `SocialPolicyGate` が宛先・権限・頻度・公開情報・秘密情報漏洩・禁止事項を検査する。却下時は `abort_proposal` を呼んで `social_proposal_aborted` (`SystemEvent`) を記録し、候補を破棄する。承認時のみ `current_state/social.json` の永続 outbox に `Authorized` として保存する。
- **Act (`kamimusuhi-discord`):**
  `kamimusuhi-discord` が応答または outbox の自発発話を Discord API へ送信し、返却された `discord_message_id` と結果を受け取る。
- **Verify / Persist (`kamimusuhi-discord` → `kamimusuhi-resident` → `kamimusuhi-runtime`):**
  `POST /v1/social/verify` により送信結果（成功時は `discord_message_id` と本文ダイジェスト、失敗時はエラー理由）を検証する。成功検証時のみ `verify_delivery` が `EvidenceKind::AgentUtterance`（`delivery: "discord_gateway_verified"`）、`mark_used`、C0 draft 提出、`memory_writeback_verified` を永続化する。送信失敗時は `abort_proposal` を呼び、`AgentUtterance` を残さない。

### 7.2 `SocialPolicyGate` 検査規則

- **発話先・DM禁止:** `guild_id` が無い DM（`SOCIAL_POLICY_DM_FORBIDDEN`）、許可外サーバー（`SOCIAL_POLICY_GUILD_MISMATCH`）、許可外チャンネル（`SOCIAL_POLICY_CHANNEL_NOT_ALLOWED`）を拒否する。自発発話は `initiative_channel_ids` に含まれるチャンネルのみ許可する（`SOCIAL_POLICY_INITIATIVE_CHANNEL_NOT_ALLOWED`）。
- **権限・自発根拠:** 応答は許可済みユーザーのみ（`SOCIAL_POLICY_USER_NOT_ALLOWED`）。自発発話は `commitment_outcome` / `delegated_review` / `research_finding` / `self_goal_progress` のいずれかの検証可能な根拠・参照・要約を必須とし（`SOCIAL_POLICY_MISSING_INITIATIVE_BASIS`）、活動量水増し（`SOCIAL_POLICY_ACTIVITY_PADDING_FORBIDDEN`）や自動的な人間関係分析（`SOCIAL_POLICY_RELATIONSHIP_PROFILING_FORBIDDEN`）を拒否する。メンション対象も許可済みユーザー／操作者に限定する（`SOCIAL_POLICY_MENTION_NOT_ALLOWED`）。
- **頻度制御:** チャンネルごとの応答クールダウン・時間上限（`SOCIAL_POLICY_REACTIVE_RATE_LIMITED`）、および自発発話のクールダウン・UTC日次上限・重複トリガー抑止（`SOCIAL_POLICY_INITIATIVE_RATE_LIMITED` / `SOCIAL_POLICY_DUPLICATE_INITIATIVE_TRIGGER`）を適用する。
- **公開情報と秘密保護:** APIキー・トークン文字列や秘密設定パスを含む候補を拒否する（`SOCIAL_POLICY_SECRET_LEAK_BLOCKED`）。公開チャンネル（`public`）では `Relationship` 記憶や非公開チャンネル（`private`）の発話履歴を想起しない。

### 7.3 `InitiativeEngine` と監督処理の独立性

- resident のバックグラウンド監督ループ（`social::spawn`）は LLM の応答停止やタイムアウトから独立して稼働し、保持期限・件数上限の監視（retention）および目標（`Commitments`）・委譲結果（`TaskPlane`）・研究知見・自己目標進捗の監視を行う。
- 生成または送信に一時障害が起きても、承認済み outbox と未検証の送達報告（`social_gateway.json` の `pending_verifications` / `pending_inbound`）は再起動を越えて保持・再試行される。

### 7.4 `Social Memory` と保持・削除制御

- **記憶型の分離:** 観測（`UserUtterance` / `SystemEvent`）、思考・生成候補（`response_generated` / `Reflection`）、検証済み発話（`AgentUtterance`）、外部事実（`LibraryExcerpt` / `ResourceResult`）、長期記憶（`state_records` / `self_model`）を型として分離する。ターン応答には `conversation_context`・`self_history`・`public_knowledge` を区別した `social_memory` メタデータを付帯する。
- **他者発言の保持制限:** 非操作者の Discord 発言は `RetentionClass::Ephemeral` として扱い、設定された TTL（`user_utterance_ttl_secs`）およびソース別件数上限（`max_user_utterances_per_channel`）を超えたものを `enforce_social_retention` により自動的に原文消去（redact）する。
- **削除要求:** Discord の `MESSAGE_DELETE` イベント、および `@澪 削除 [<message_id>]` / `POST /v1/social/delete` / `kamimusuhi-runtime social-redact` により、`redact_discord_utterance` が SQLite の対象 `payload_json` から `"text"` を消去し、`EvidenceKind::Correction`（`EvidenceRelation::Corrects`）を追加し、該当根拠に依存する `state_records` を `invalidated` に更新する。以降、派生する `AgentUtterance` や想起候補からも除外される。

### 7.5 実環境での完了検証チェックリスト（§6）

以下は本番環境（Pi / 実 Discord サーバー）への配置承認後に実施し、すべて確認した時点で社会的活動基盤として完成と判定する。

1. **再起動後の継続:** resident / discord-gateway の再起動を跨いで、`current_state/social.json`・`social_gateway.json`・`kamimusuhi.sqlite` の目標と未検証送達が引き継がれること。
2. **権限分離:** 許可外ユーザー・許可外チャンネル・DM・公開チャンネルでの私的記憶想起・秘密情報漏洩・根拠なし自発発話が `SocialPolicyGate` により拒否されること。
3. **障害復旧:** Discord API 送達失敗時の `abort_proposal`（`AgentUtterance` 未記録）、および送信成功直後の resident 断絶からの `pending_verifications` 再送・冪等永続化が実環境で確認できること。
4. **削除反映:** Discord 上のメッセージ削除および `@澪 削除` により SQLite の原文が redact され、以降の想起・返答に現れないこと。
