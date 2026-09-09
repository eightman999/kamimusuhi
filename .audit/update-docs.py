from pathlib import Path
import re

root = Path('.')

def replace(name, old, new):
    path = root / name
    text = path.read_text()
    assert text.count(old) == 1, (name, text.count(old), old[:100])
    path.write_text(text.replace(old, new))

for name in ('spec.md', 'spec.en.md'):
    replace(name, 'Implementation target: **v0.1 continuity slice**  ',
            'Implementation target: **v0.1 continuity slice**  \nImplementation-boundary clarification: **2026-09-10 / W5–W7 hardening**\\')

replace('spec.md', '### NFR-009 — Source-of-truth discipline', '''#### NFR-008A — Provider 境界の具体化（2026-09-10）

- turn に適用する privacy/locality 制約は、delegated resource だけでなく、記憶・Library・入力を受け取る Persona backend の dispatch にも MUST 適用する。別 namespace であることは外部送信の例外にならない。locality が未宣言の場合は安全側に扱い、URL 文字列だけを locality の実測証拠としてはならない。
- Persona input の serialization は、利用する workspace item の domain、authority、source/evidence refs、freshness、および continuity/session の文脈を MUST 保持する。payload 内の改行・見出しを構造上の authority と解釈してはならない。これはモデルの意味的な prompt-injection 耐性を証明する要件ではない。
- provider 応答は、transport framing と設定されたサイズ境界を MUST 検証する。不完全・不正・過大な応答から final expression や canonical activation を作ってはならない。
- provider が返した任意の error field、認証値、反射された入力を diagnostics に直接保存してはならない。分類は固定 code 等の bounded metadata へ正規化する MUST。原証拠の意図的な保存とは別境界である。
- retry/backoff は logical call の timeout budget を共有する MUST。各 attempt で budget を再発行してはならない。DNS 等の実装上中断できない待機が残る場合は、その制約を明示する MUST。

### NFR-009 — Source-of-truth discipline''')
replace('spec.en.md', '### NFR-009 — Source-of-truth discipline', '''#### NFR-008A — Provider-boundary clarification (2026-09-10)

- A turn's privacy/locality constraint MUST apply to Persona dispatch receiving input, memory, and Library material, not only to delegated resources. A separate namespace grants no export exception. Undeclared locality MUST fail conservatively; a URL alone is not measured locality evidence.
- Persona input serialization MUST retain the used workspace items' domain, authority, source/evidence references, freshness, and continuity/session context. Newlines or headings inside a payload MUST NOT acquire structural authority. This requirement does not establish semantic prompt-injection resistance in the model.
- Provider responses MUST pass transport-framing and configured size checks. Incomplete, malformed, or oversized responses MUST NOT produce a final expression or canonical activation.
- Arbitrary provider error fields, credentials, and reflected input MUST NOT be copied directly into diagnostics. Classification MUST use bounded metadata such as fixed codes. Intentional canonical-evidence storage is a separate boundary.
- Retries and backoff MUST share one logical-call timeout budget rather than receiving a fresh budget per attempt. Implementations MUST disclose remaining non-interruptible waits such as synchronous DNS.

### NFR-009 — Source-of-truth discipline''')
replace('spec.md', '## 16. Planned milestones', '''### 現実装への適用と確認範囲（2026-09-10）

W0–W7 は v0.1 continuity slice と router/TLS/Persona backend の基盤を実装する。これは本仕様の将来 FR 全体への適合宣言ではない。専用 Persona 学習、self domain mutation、belief graph、常時背景認知、分散身体等は未実装のままである。

W5–W7 の追加監査・回帰テスト・transport 制約は [実装監査結果](./docs/implementation/2026-09-10-spec-implementation-audit.md) に記録する。HTTP fixture による契約検証と実 LLM による能力・人格評価を区別し、後者の完了は主張しない。

## 16. Planned milestones''')
replace('spec.en.md', '## 16. Planned milestones', '''### Current implementation and verification scope (2026-09-10)

W0–W7 implement the v0.1 continuity slice plus router/TLS/Persona-backend foundations. This is not a claim of conformance to every future functional requirement. Dedicated Persona training, self-domain mutation, belief graphs, persistent background cognition, and distributed embodiment remain unimplemented.

See the [implementation audit](./docs/implementation/2026-09-10-spec-implementation-audit.md) for W5–W7 hardening, regression tests, and transport limits. HTTP-fixture contract tests are distinct from real-LLM capability/persona evaluation; the latter is not claimed complete.

## 16. Planned milestones''')

replace('README.md', 'モデルに渡される prompt は section 付きです。`CURRENT_INPUT` / `RELATIONSHIP_MEMORY` / `LIBRARY_EVIDENCE` / `EXTERNAL_RESOURCE_RESULT` がそれぞれ何であるかを明示して渡すので、Library の文章や外部 resource の出力が「自分の記憶」や「事実」として混ざりません。token が必要な endpoint では、runtime.json の `persona.provider.auth_env` に**環境変数名**を書きます（値は書きません）。', '''モデルへは、section と JSON record で domain・authority・source/evidence refs・freshness を保った入力を渡します。`CURRENT_INPUT`、記憶、Library、外部結果に加え、`CONTINUITY_STATE` / `SESSION_WORKING_STATE` と turn/input の識別子も渡します。payload 内の改行や偽の見出しは JSON string に閉じ込めますが、これだけで **LLM の意味的な prompt injection や誤解釈を防げるとは主張しません**。永続状態への書込みは別の Mutation Policy / Continuity Kernel 境界です。

token が必要な endpoint では、runtime.json の `persona.provider.auth_env` に**環境変数名**を書きます（値は書きません）。Persona の外部送信にも `--privacy` が適用されます。locality 未指定の既存設定は `external` とみなし、`local-only` での暗黙の許可はしません。

```bash
# 上の demo で作った .local/demo を使用。ローカルで実際に管理する endpoint のみ宣言する。
cargo run -p kamimusuhi-runtime -- demo-continuity --dir .local/demo --phase resume \\
  --id-seed 30 --persona openai-compatible \\
  --persona-url http://127.0.0.1:11434/v1 --persona-model llama3.2 \\
  --persona-locality local-host --privacy local-only
```

`--persona-locality` は `local-host` / `local-network` / `external` の **operator 宣言**であり、実測・attestation ではありません。永続設定は `persona.provider.locality`（`local_host` 等）です。Persona の URL を交換すると旧 endpoint の認証・private CA・locality は自動継承しません。必要な設定は新しい送信先に対して明示してください。

**検証範囲:** 自動テストは scripted HTTP/TLS endpoint と別プロセスでの境界検証です。実 LLM server に対する smoke test と能力・人格評価の実測は未記録です。transport のサイズ上限、対応範囲、DNS の制約は [監査結果](./docs/implementation/2026-09-10-spec-implementation-audit.md) を参照してください。''')
replace('README.md', 'テストと lint は次で回します。', 'テストと lint は次で回します。GitHub Actions も同じ script を実行します。')
replace('README.md', '実装範囲は W0–W5:', '実装範囲は W0–W7:')
replace('README.md', '- W7: Persona Core 境界の強化。実モデル backend、typed input envelope、外部 material と最終応答の分離', '- W7: Persona Core 境界の強化。OpenAI-compatible backend、typed input envelope、外部 material と最終応答の分離。2026-09-10 に privacy / transport / attribution 境界を追加修正')
replace('README.md', '（W6）を参照してください。', '（W6）、[W7 実装結果](./docs/implementation/w7-persona-core-plan.md)、[2026-09-10 監査結果](./docs/implementation/2026-09-10-spec-implementation-audit.md) を参照してください。')

replace('README.en.md', '''The prompt the model receives is sectioned: `CURRENT_INPUT`,
`RELATIONSHIP_MEMORY`, `LIBRARY_EVIDENCE` and `EXTERNAL_RESOURCE_RESULT` are
labelled for what they are, so Library text and resource output cannot arrive
as the individual's own memory or as fact. For an endpoint that needs a token,
put the *name* of the environment variable in `persona.provider.auth_env`; the
value is never written to the config, the database or the trace.''', '''Sections and JSON records preserve domain, authority, source/evidence references,
and freshness. The backend receives `CONTINUITY_STATE`, `SESSION_WORKING_STATE`,
and turn/input identifiers as well as current input, memory, Library, and
external results. Payload newlines and forged headings stay inside JSON strings.
This is **not a claim of semantic prompt-injection resistance in an LLM**;
durable writes still require the separate Mutation Policy / Continuity Kernel.

For authentication, put the environment variable's *name*, not its value, in
`persona.provider.auth_env`. `--privacy` applies to Persona dispatch too.
Legacy configurations without locality default to `external`, not an implicit
permission to export under `local-only`.

```bash
# Reuse .local/demo from the demo above. Declare only an endpoint you control locally.
cargo run -p kamimusuhi-runtime -- demo-continuity --dir .local/demo --phase resume \\
  --id-seed 30 --persona openai-compatible \\
  --persona-url http://127.0.0.1:11434/v1 --persona-model llama3.2 \\
  --persona-locality local-host --privacy local-only
```

`--persona-locality` accepts `local-host`, `local-network`, or `external`.
It is an **operator declaration**, not measurement or attestation. Persist it
as `persona.provider.locality` (`local_host`, etc.). Changing the Persona URL
no longer inherits the old endpoint's credentials, private CA, or locality;
configure these explicitly for the new destination.

**Verification scope:** automated tests exercise scripted HTTP/TLS endpoints
and real process boundaries, not an actual LLM server. Real-model smoke runs
and capability/persona evaluations remain unrecorded. See the [audit](./docs/implementation/2026-09-10-spec-implementation-audit.md)
for transport limits and the synchronous-DNS caveat.''')
replace('README.en.md', 'Tests and lints:', 'Tests and lints (GitHub Actions runs this same script):')
replace('README.en.md', 'Waves W0–W5:', 'Waves W0–W7:')
replace('README.en.md', '- W7: Persona Core boundary hardening — a real model backend, a typed input envelope, and external material kept separate from the final expression', '- W7: Persona Core boundary hardening — an OpenAI-compatible backend, a typed input envelope, and external material kept separate from the final expression; privacy, transport, and attribution hardened on 2026-09-10')
replace('README.en.md', '(W6) for exactly what was demonstrated and what the known limits are.', '(W6), the [W7 result](./docs/implementation/w7-persona-core-plan.md), and the [2026-09-10 audit](./docs/implementation/2026-09-10-spec-implementation-audit.md) for demonstrated behavior and known limits.')

replace('docs/README.md', '### Implementation plans', '''### Implementation plans and results

- [`implementation/2026-09-10-spec-implementation-audit.md`](./implementation/2026-09-10-spec-implementation-audit.md) — **仕様対実装の監査と修正結果**。Persona privacy、HTTP framing/size、safe diagnostics、完全な input provenance、logical-call deadline の修正。実行した CI と mutation test、実モデル未検証、残る制約を区別する。''')
replace('docs/README.md', 'Persona Core 境界の強化と実モデル backend。', 'Persona Core 境界の強化と OpenAI-compatible backend（自動検証は scripted endpoint）。')
replace('docs/README.md', '**第二次実装計画**。W6（Cognitive Resource Router と TLS）の acceptance criteria と、W7 以降へ進まない境界を明記する。計画であり実装済みではない。', '**第二次実装計画と W6 実装結果**。Cognitive Resource Router / TLS の acceptance と当時の wave 境界を記録。後半に実装結果があり、W7 は別の計画・結果文書を参照する。')
replace('docs/README.md', '現時点では計画であり実装済みではない。', '当時の計画であり、実装の達成範囲は別の phase-1 implementation result を参照する。')

w7 = 'docs/implementation/w7-persona-core-plan.md'
replace(w7, '# W7 — Persona Core boundary hardening and a real Persona model', '''# W7 — Persona Core boundary hardening and a model-capable Persona backend

> Verification correction (2026-09-10): transport and process-boundary tests use
> scripted endpoints. No actual LLM-server smoke run is recorded. Historical
> counts below describe the W7 baseline; subsequent fixes are documented in the
> [spec/implementation audit](./2026-09-10-spec-implementation-audit.md).''')
replace(w7, '- [x] One turn succeeds against a real local OpenAI-compatible endpoint.', '''- [x] One turn succeeds through a local scripted OpenAI-compatible HTTP endpoint.
- [ ] One turn succeeds against an actual local LLM server, with model/version and
      run evidence recorded. This is not established by an HTTP fixture.''')
replace(w7, '- [x] Fake and real Persona Cores go through the same `PersonaCore` boundary.', '- [x] Fake and HTTP-backed Persona Cores go through the same `PersonaCore` boundary.')
replace(w7, '      domain intact, and is only flattened at serialization.', '''      domain intact, and is only flattened at serialization. Complete continuity,
      session, authority, freshness, and source fields were repaired on 2026-09-10;
      the original W7 renderer did not preserve all of them.''')
replace(w7, '- [x] No secret, prompt or response body in database, trace or error.', '''- [x] Credentials and raw provider-error/prompt bodies are not copied into
      diagnostics. Canonical user evidence and attributed successful resource
      material are intentionally stored through their own evidence paths;
      this is not a claim that the whole database contains no text.''')
replace(w7, 'only HTTP and TLS transport. `OpenAiCompatiblePersona` implements `PersonaCore`', 'HTTP/TLS transport and, since 2026-09-10, bounded provider-error classification.\n`OpenAiCompatiblePersona` implements `PersonaCore`')
replace(w7, 'A real turn against a local OpenAI-compatible endpoint, driven by the CLI:', 'A CLI turn against a scripted local OpenAI-compatible endpoint (not an actual LLM):')
replace(w7, 'expression         model-generated, in Japanese, citing the stored preference', 'expression         scripted Japanese reply mentioning the stored preference')
replace(w7, 'Tests: **266 passing, 0 failed, 0 ignored**', 'Historical W7 baseline tests: **266 passing, 0 failed, 0 ignored**')
replace(w7, 'labels each section and each item by the ID it can be traced back to. `durable_self`', 'now preserves complete item metadata as JSON records (2026-09-10 correction).\n`durable_self`')

replace('docs/implementation/phase-2-implementation-plan.md', '''A fourth behaviour was deliberate rather than a bug: a peer that closes without
`close_notify` is tolerated, because real servers do it constantly. The cost is
stated in the code — with `Connection: close` framing a truncated body cannot
be told from a complete one, so truncation surfaces as a parse failure rather
than being silently accepted.''', '''A peer closing without TLS `close_notify` is tolerated. **Correction, 2026-09-10:**
the original claim that truncation necessarily surfaced as a parse failure was
incorrect: a truncated body could still be valid JSON, and Content-Length was
not checked. The hardened transport now requires exact Content-Length or a
complete chunked message; EOF alone is not a completeness signal. See the
[regression audit](./2026-09-10-spec-implementation-audit.md).''')

http = 'crates/kamimusuhi-resource-http/src/http.rs'
replace(http, '/// Path with a leading slash, query included.', '/// Path with a leading slash. Query strings and fragments are rejected.')
replace(http, '/// POST a JSON body and read the response, all within `timeout`.', '/// POST a JSON body with socket work bounded by a shared `timeout` deadline.\n/// OS name resolution is synchronous and cannot be interrupted by this client.')
replace(http, '''            // A TLS peer that closes without `close_notify` is common enough
            // in the wild that refusing to read such a response would fail
            // against real servers. The cost is that an unclean EOF cannot be
            // told from a clean one here — a truncated body therefore surfaces
            // as a malformed response when it fails to parse, rather than
            // being quietly accepted as complete.''', '''            // Tolerate missing TLS close_notify only when HTTP framing proves
            // the response is complete. parse_response requires exact length
            // or a complete chunked terminator; valid JSON alone is not enough.''')

for name in ('README.md', 'README.en.md', 'spec.md', 'spec.en.md', 'docs/README.md',
             'docs/implementation/w7-persona-core-plan.md',
             'docs/implementation/phase-2-implementation-plan.md',
             'docs/implementation/2026-09-10-spec-implementation-audit.md'):
    path = root / name
    for target in re.findall(r'\]\(([^\s)]+)\)', path.read_text()):
        if ':' not in target and not target.startswith('#'):
            assert (path.parent / target.split('#')[0]).exists(), (name, target)
print('Documentation replacements and local markdown links verified')
