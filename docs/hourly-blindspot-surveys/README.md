# Hourly Blind-Spot Surveys

Kamimusuhi の通常の分野横断 daily survey では拾いにくい、隣接分野・異分野・実装基盤・標準・安全工学・人文学/社会科学などの設計知を補完するための hourly research intake。

このディレクトリの内容は **non-normative**。`spec.md` / `architecture.md` を自動的に変更せず、重要項目はレビュー後に research foundation / watchlist / subsystem note / specification へ昇格する。

## Policy

- `docs/daily-surveys/` と過去24時間の本ディレクトリを確認して重複を避ける。
- 過去1〜24時間の一次資料・公式資料を優先する。
- Zenn / note / Qiita も実装知見・日本語圏の着想を拾う discovery source として探索し、一般化された技術・性能・研究主張は可能な限り一次資料で裏取りする。
- 新着が薄い場合のみ、Kamimusuhi に未収載の重要な過去研究・古典・異分野概念を少数補完する。
- 「実証済み」と「提案・推測」を分ける。
- Kamimusuhi との差分と、借りるべき設計契約を明示する。
- 有意な発見がない場合も、検索した領域と `有意な新規発見なし` を記録する。

## 反映判断

[2026-09-11 判定台帳](../research/survey-decisions-2026-09-11.md)に、文書別の採否・理由・Issue反映先・保留条件・判断対象blobを記録した。**`済✅️` は反映するか否かの判断完了であり、全提案の採用・実装完了・論文の全面検証を意味しない。** 原文の研究記録は変更しない。

新規サーベイは `未判定` とする。既存Issue/spec/READMEとの重複を確認し、採用・既存反映済み・保留・不採用を決め、採用する書込みを確認してから `済✅️` を付ける。原文blobや判断に重要なevidence/実装条件が変わった場合は再判定する。過去の完了マークを新規findingへ継承せず、S/A評価だけで規範へ自動昇格させない。

## Index

| Date / time (JST) | Survey | Main blind spots | 反映判断 |
|---|---|---|---|
| 2026-09-12 18:59:03 | [2026-09-12-18-59-03.md](./2026-09-12-18-59-03.md) | deterministic replay / software-version continuity / mixed-criticality cognition / decision-oriented freshness / exact learning-trace fidelity / recomputable effect-admission verdicts | 未判定 |
| 2026-09-12 13:02:29 | [2026-09-12-13-02-29.md](./2026-09-12-13-02-29.md) | ambient/stigmergic communication channels / operational skill resurrection after revocation / capability-attenuation implementation evidence / resource-instance identity across restart | 未判定 |
| 2026-09-12 07:00:50 | [2026-09-12-07-00-50.md](./2026-09-12-07-00-50.md) | runtime spec-implementation conformance / source-bound TEE attestation / deployed model-state closure / uncertain sensor attribution / speculative-decoding integrity telemetry / recurrent coordination safety | 未判定 |
| 2026-09-12 00:57:53 | [2026-09-12-00-57-53.md](./2026-09-12-00-57-53.md) | latent interface portability / blast-radius admission / executable transition memory / bounded residual reflex learning / functional degeneracy / weight-level privacy / resource-coupled cooperation | 未判定 |
| 2026-09-11 19:01:12 | [2026-09-11-19-01-12.md](./2026-09-11-19-01-12.md) | abort liveness / effective environment attestation / transitive skill dependency closure / prospective memory / successful strategy coverage / regulatory awareness clocks | 未判定 |
| 2026-09-11 13:01:43 | [2026-09-11-13-01-43.md](./2026-09-11-13-01-43.md) | actor-native identity / decision-evidence binding / verifiable retrieval / implicit conventions / persistence pressure / high-fanout sandbox memory / noisy physical cognition / embodiment-bound skills | 未判定 |
| 2026-09-11 07:00:48 | [2026-09-11-07-00-48.md](./2026-09-11-07-00-48.md) | verifiable outsourced cognition / latent competence telemetry / differentiated trust / correlated cognitive dependency / trajectory tamper localization / bitemporal memory semantics | [済✅️](../research/survey-decisions-2026-09-11.md#b06) |
| 2026-09-11 00:58:49 | [2026-09-11-00-58-49.md](./2026-09-11-00-58-49.md) | conclusion-contaminated audit / unreliable-tool overtrust / individuality loss in personalization / adaptive transcript privacy / effect-conditioned safety metrics / portable runtime evidence | [済✅️](../research/survey-decisions-2026-09-11.md#b05) |
| 2026-09-10 19:01:16 | [2026-09-10-19-01-16.md](./2026-09-10-19-01-16.md) | verification adequacy / pre-cognition bootstrap trust / restore-counterfactual forgetting audit / skill trace contracts / compute-cache invalidation / portable Library trust metadata | [済✅️](../research/survey-decisions-2026-09-11.md#b04) |
| 2026-09-10 12:59:38 | [2026-09-10-12-59-38.md](./2026-09-10-12-59-38.md) | storage-vs-use forgetting / lifecycle metadata / memory access kernel / cognitive stopping / relational affect / partial-TEE trust | [済✅️](../research/survey-decisions-2026-09-11.md#b03) |
| 2026-09-10 06:58:43 | [2026-09-10-06-58-43.md](./2026-09-10-06-58-43.md) | effect acceptance / accountability roles / presentation identity / OpenTelemetry / trajectory databases | [済✅️](../research/survey-decisions-2026-09-11.md#b02) |
| 2026-09-10 00:57:57 | [2026-09-10-00-57-57.md](./2026-09-10-00-57-57.md) | object capabilities / runtime assurance / privacy-aware memory / PROV-O / CRDT state classification | [済✅️](../research/survey-decisions-2026-09-11.md#b01) |