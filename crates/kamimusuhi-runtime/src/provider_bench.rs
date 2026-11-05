//! Multi-provider latency comparison and shadow-routing foundation.
//!
//! This module exists to answer one question with measurements, not guesses:
//! which OpenAI-compatible endpoint should serve which kind of Kamimusuhi
//! turn. It is also where the Jev route-classification contract lives — a
//! small typed decision (FAST_CHAT / LOCAL_CHAT / DEEP_REASONING /
//! TOOL_TASK / MEMORY_HEAVY) that can run in parallel with prompt assembly
//! later, and that today only *records* where Jev would have routed.
//!
//! Provider-specific knowledge is data, not code: every OpenAI-compatible
//! provider shares [`BenchClient`]. Only Jev is different — it is not a
//! language organ at all but the TypeSafe `/v1/systemone` decision endpoint,
//! used here as the routing probe.
//!
//! Secret discipline, same as the rest of the crate: bearer tokens arrive
//! from operator-named key files or environment variables, are held only in
//! process memory, and are never written to the report, the trace, errors or
//! stdout. Error classification uses the existing [`HttpError`] / status
//! vocabulary, so a bench sample carries no response text either.
//!
//! Cost discipline: every request is estimated before it is sent and refused
//! when the estimate exceeds the per-request cap, and the run halts at the
//! total cap. Providers that report billed cost (OrcaRouter's
//! `usage.cost_usd`, OpenRouter's `usage.cost`) are recorded as actuals;
//! everything else is an estimate from the static price table and is marked
//! as such.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use kamimusuhi_core::ids::{EvidenceId, IndividualId, SessionId, TurnId};
use kamimusuhi_core::persona::{
    ConversationMessage, ConversationRole, CurrentInput, PersonaTurnInput, TurnContext,
};
use kamimusuhi_core::persona_seed::{V0_SEED_ID, v0_seed};
use kamimusuhi_persona_http::{OpenAiCompatiblePersona, PersonaBackendConfig};
use kamimusuhi_resource_http::http::{get_json, post_json};
use kamimusuhi_resource_http::{Endpoint, Header, HttpError, TrustAnchors, post_json_stream};
use serde::Serialize;

use crate::dialogue_setup::persona_backend_id_for;
use crate::llm_jev::{
    ConversationCoreState, DEFAULT_HAI_BASE_URL, HAI_API_KEY_ENV, HAI_LLM_JP_MODEL, HAI_QWEN_MODEL,
    TypesafeConfig, infer_speech_act,
};
use crate::route_gate::BillingClass;

/// Per-request estimated-cost cap: a call whose estimate exceeds this is
/// never sent.
pub const DEFAULT_REQUEST_COST_CAP_USD: f64 = 0.02;
/// Whole-run cost cap: the benchmark stops before spending more than this.
pub const DEFAULT_TOTAL_COST_CAP_USD: f64 = 0.50;
/// Replies are 1-3 short sentences; the cap bounds cost, not expression.
/// Reasoning models can spend part of the budget thinking — the cap leaves
/// headroom so a thinking model still gets to answer.
pub const DEFAULT_MAX_COMPLETION_TOKENS: u32 = 768;
/// Mio's ordinary short turns, including the latency complaint that
/// motivated this work.
pub const BENCH_PROMPTS: [(&str, &str); 3] = [
    ("greeting", "こんばんは"),
    ("closing_report", "今日も終わるからご挨拶をと思ってね"),
    (
        "latency_question",
        "もう少しお返事が早くならないかと思ってるんだけどどうかな",
    ),
];

/// Build the `/chat/completions` body for one benchmark prompt.
///
/// With `persona_style` the body is produced by the *same* code the dialogue
/// path uses — [`OpenAiCompatiblePersona::request_body`] — with the seed, a
/// small standing conversation, an observed-runtime block and the derived
/// conversation core. The result is what providers actually have to carry,
/// so the measured prompt tokens reflect Mio's real context, not a toy
/// prompt. With `persona_style` off, the body is a single user message.
///
/// The body has no `stream`/`max_tokens` yet; the caller adds them per
/// request so streamed and buffered calls stay comparable.
pub fn prompt_body(
    spec: &ProviderSpec,
    model: &str,
    user_text: &str,
    persona_style: bool,
) -> serde_json::Value {
    if !persona_style {
        return serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": user_text}],
        });
    }
    let persona = OpenAiCompatiblePersona::new(PersonaBackendConfig::new(
        persona_backend_id_for(spec.base_url, model),
        spec.base_url,
        model,
    ));
    let core_state =
        ConversationCoreState::for_input(&ConversationCoreState::default(), 0, user_text);
    let mut input = PersonaTurnInput::bare(
        TurnContext {
            individual_id: IndividualId::from_u128(0x4b_01),
            session_id: SessionId::from_u128(0x4b_02),
            turn_id: TurnId::from_u128(0x4b_03),
        },
        CurrentInput {
            evidence_id: EvidenceId::from_u128(0x4b_04),
            text: user_text.to_owned(),
        },
    );
    input.envelope.persona_seed = Some(v0_seed(V0_SEED_ID));
    input.envelope.conversation_history = vec![
        ConversationMessage {
            evidence_id: EvidenceId::from_u128(0x4b_05),
            role: ConversationRole::User,
            text: "こんばんは、今日は少し長い一日でした".to_owned(),
        },
        ConversationMessage {
            evidence_id: EvidenceId::from_u128(0x4b_06),
            role: ConversationRole::Assistant,
            text: "こんばんは。おつかれさま、ゆっくりしてね".to_owned(),
        },
    ];
    input.envelope.observed_runtime = Some(serde_json::json!({
        "interface": "text",
        "history_messages_available": 2,
        "retained_memory_records_available": 0,
        "conversation_core_state": core_state,
        "speech_output": "not_connected",
        "body_sensors": "not_connected_to_this_interface",
        "experimental_neural_state": "not_connected_to_this_interface",
    }));
    input.envelope.conversation_core = serde_json::to_value(&core_state).ok();
    let mut body = serde_json::from_str::<serde_json::Value>(&persona.request_body(&input))
        .unwrap_or_else(|_| {
            serde_json::json!({
                "model": model,
                "messages": [{"role": "user", "content": user_text}],
            })
        });
    // vLLM-style servers take `chat_template_kwargs`; strict endpoints
    // reject the unknown field, so it comes off for them. The prompt text —
    // the part that costs latency — is identical either way.
    if !spec.accepts_chat_template_kwargs
        && let Some(map) = body.as_object_mut()
    {
        map.remove("chat_template_kwargs");
    }
    body
}

/// A persona-style body padded to approximately `target_tokens` of prompt —
/// production turns run 16–28k tokens, so sized bodies measure what a real
/// long-context turn costs each provider. Padding lands in
/// `conversation_history`, the section that carries real deployments'
/// bulk; filler turns are synthetic small talk, fixed and repetitive so the
/// provider's tokenizer sees realistic Japanese, not entropy.
pub fn prompt_body_sized(
    spec: &ProviderSpec,
    model: &str,
    user_text: &str,
    target_tokens: u64,
) -> serde_json::Value {
    const FILLER_USER: &str = "今日は少し疲れた一日でした。仕事のあとで買い物に寄って、夕食の材料を買ってきました。帰り道の空がきれいでした。";
    const FILLER_ASSISTANT: &str =
        "おつかれさまです。ゆっくり休んでくださいね。夕食は何を作る予定ですか。";

    // Build the same input prompt_body builds, then pad history toward the
    // target before rendering.
    let persona = OpenAiCompatiblePersona::new(PersonaBackendConfig::new(
        persona_backend_id_for(spec.base_url, model),
        spec.base_url,
        model,
    ));
    let core_state =
        ConversationCoreState::for_input(&ConversationCoreState::default(), 0, user_text);
    let mut input = PersonaTurnInput::bare(
        TurnContext {
            individual_id: IndividualId::from_u128(0x4b_01),
            session_id: SessionId::from_u128(0x4b_02),
            turn_id: TurnId::from_u128(0x4b_03),
        },
        CurrentInput {
            evidence_id: EvidenceId::from_u128(0x4b_04),
            text: user_text.to_owned(),
        },
    );
    input.envelope.persona_seed = Some(v0_seed(V0_SEED_ID));
    input.envelope.conversation_history = vec![
        ConversationMessage {
            evidence_id: EvidenceId::from_u128(0x4b_05),
            role: ConversationRole::User,
            text: "こんばんは、今日は少し長い一日でした".to_owned(),
        },
        ConversationMessage {
            evidence_id: EvidenceId::from_u128(0x4b_06),
            role: ConversationRole::Assistant,
            text: "こんばんは。おつかれさま、ゆっくりしてね".to_owned(),
        },
    ];
    input.envelope.observed_runtime = Some(serde_json::json!({
        "interface": "text",
        "history_messages_available": 2,
        "retained_memory_records_available": 0,
        "conversation_core_state": core_state,
        "speech_output": "not_connected",
        "body_sensors": "not_connected_to_this_interface",
        "experimental_neural_state": "not_connected_to_this_interface",
    }));
    input.envelope.conversation_core = serde_json::to_value(&core_state).ok();

    // Pad toward the target. Estimate tokens as bytes/3 (conservative for
    // Japanese UTF-8); converge by adding filler pairs in batches.
    let render = |input: &PersonaTurnInput| -> serde_json::Value {
        let mut body = serde_json::from_str::<serde_json::Value>(&persona.request_body(input))
            .unwrap_or_else(|_| {
                serde_json::json!({
                    "model": model,
                    "messages": [{"role": "user", "content": user_text}],
                })
            });
        if !spec.accepts_chat_template_kwargs
            && let Some(map) = body.as_object_mut()
        {
            map.remove("chat_template_kwargs");
        }
        body
    };
    let pair_tokens = ((FILLER_USER.len() + FILLER_ASSISTANT.len() + 64) / 3) as u64;
    let mut body = render(&input);
    let mut est = (body.to_string().len() / 3) as u64;
    let mut next_id = 0x4b_10u128;
    while est < target_tokens {
        let deficit = target_tokens - est;
        let pairs = (deficit / pair_tokens.max(1)).clamp(1, 50);
        for _ in 0..pairs {
            for (role, text) in [
                (ConversationRole::User, FILLER_USER),
                (ConversationRole::Assistant, FILLER_ASSISTANT),
            ] {
                input
                    .envelope
                    .conversation_history
                    .push(ConversationMessage {
                        evidence_id: EvidenceId::from_u128(next_id),
                        role,
                        text: text.to_owned(),
                    });
                next_id += 1;
            }
        }
        body = render(&input);
        est = (body.to_string().len() / 3) as u64;
    }
    body
}

/// Where a provider's bearer token comes from. The registry stores the
/// *location* of the credential; the value only ever exists in [`KeyStore`]
/// memory or the process environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSource {
    /// An environment variable name (the existing provider convention).
    Env(String),
    /// A file supplied via `--key <provider>:<path>` at run time.
    KeyFile,
    /// The endpoint needs no credential.
    None,
}

/// One model a provider might serve. Prices are USD per 1M tokens from the
/// provider's published list, `None` when the provider does not publish one —
/// an unknown price is reported as such rather than guessed.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    pub input_usd_per_mtok: Option<f64>,
    pub output_usd_per_mtok: Option<f64>,
}

const fn priced(id: &'static str, input: f64, output: f64) -> ModelSpec {
    ModelSpec {
        id,
        input_usd_per_mtok: Some(input),
        output_usd_per_mtok: Some(output),
    }
}

const fn unpriced(id: &'static str) -> ModelSpec {
    ModelSpec {
        id,
        input_usd_per_mtok: None,
        output_usd_per_mtok: None,
    }
}

/// Non-secret description of one provider. Everything here may be committed;
/// nothing here is a credential.
#[derive(Debug, Clone)]
pub struct ProviderSpec {
    /// Stable registry id, used on `--providers` and in report rows.
    pub id: &'static str,
    /// `https://host/v1`-style base; `/chat/completions` and `/models` are
    /// appended to it. Empty for providers that are declared but configured
    /// elsewhere (the llm_master tier lives in deploy-local config).
    pub base_url: &'static str,
    pub auth: AuthSource,
    /// Preferred models, cheapest first. When the provider answers
    /// `GET /models`, the first available candidate wins; otherwise the
    /// first candidate is used as-is.
    pub models: &'static [ModelSpec],
    /// Static, non-secret request headers (e.g. OrcaRouter cost reporting).
    pub extra_headers: &'static [(&'static str, &'static str)],
    /// Extra top-level fields merged into the request body, as a JSON
    /// object literal (e.g. OpenRouter's `{"usage":{"include":true}}`).
    /// `""` when the provider needs none. A literal keeps the registry
    /// fully static; it is parsed once per request.
    pub extra_body: &'static str,
    /// Whether `stream_options.include_usage` is understood.
    pub stream_usage: bool,
    /// Whether `chat_template_kwargs` may be sent. vLLM-style servers use it
    /// (`enable_thinking`); strict OpenAI endpoints reject it with a 400, so
    /// it is stripped for them — the prompt itself is unchanged.
    pub accepts_chat_template_kwargs: bool,
    /// Key under which [`KeyStore`] holds this provider's token. Almost
    /// always the provider id; a derived spec (e.g. `orcarouter-auto`) names
    /// the base provider here so the same credential is reused.
    pub key_name: &'static str,
    /// How the declared plan is billed — the route gate's cost axis.
    pub billing: BillingClass,
    /// Whether prompts carrying persona memory, private history, durable
    /// self or private references may be sent. `false` for plans whose free
    /// use permits model improvement on inputs: such providers still serve
    /// benchmark prompts and public tasks, never private ones.
    pub privacy_ok_for_private_memory: bool,
    /// Maximum prompt tokens the provider accepts, when known.
    pub context_limit_tokens: Option<u32>,
    /// Approximate per-minute token budget (free tiers publish one). A
    /// prompt over this is routed elsewhere instead of eaten by a 429.
    pub approx_tpm_limit: Option<u64>,
    /// Whether reasoning ("thinking") can be suppressed on this provider —
    /// directly affects TTFT for fast-chat turns.
    pub reasoning_suppression: bool,
    /// When `Some`, the provider is declared but never probed — the
    /// explanation is part of the record.
    pub skip_reason: Option<&'static str>,
}

/// The existing primary: HAI's hosted endpoint, same models the dialogue
/// path declares. Pricing is not published to this client; usage is still
/// recorded so cost can be reconciled out-of-band.
const HAI: ProviderSpec = ProviderSpec {
    id: "hai",
    base_url: DEFAULT_HAI_BASE_URL,
    auth: AuthSource::Env(String::new()), // filled by registry() with HAI_API_KEY_ENV
    models: &[unpriced(HAI_QWEN_MODEL), unpriced(HAI_LLM_JP_MODEL)],
    extra_headers: &[],
    extra_body: "",
    stream_usage: true,
    accepts_chat_template_kwargs: true,
    key_name: "hai",
    billing: BillingClass::Subscription,
    privacy_ok_for_private_memory: true,
    context_limit_tokens: None,
    approx_tpm_limit: None,
    reasoning_suppression: true,
    skip_reason: None,
};

/// The local llama node behind the peer resident. Declared so the adapter
/// shape covers it, but it is off-duty overnight and never probed here.
const LLM_MASTER: ProviderSpec = ProviderSpec {
    id: "llm_master",
    base_url: "",
    auth: AuthSource::Env(String::new()),
    models: &[],
    extra_headers: &[],
    extra_body: "",
    stream_usage: false,
    accepts_chat_template_kwargs: true,
    key_name: "llm_master",
    billing: BillingClass::Local,
    privacy_ok_for_private_memory: true,
    // llama-server is configured with a 65,536-token context in deploy.
    context_limit_tokens: Some(65_536),
    approx_tpm_limit: None,
    reasoning_suppression: true,
    skip_reason: Some(
        "node is off-duty (night break); peer-local tier, configured in deploy/resident/local",
    ),
};

const CEREBRAS: ProviderSpec = ProviderSpec {
    id: "cerebras",
    base_url: "https://api.cerebras.ai/v1",
    auth: AuthSource::KeyFile,
    // Published list prices (USD/1M tokens). This account's catalog only
    // exposes the two models below; qwen's price is not published to us.
    models: &[priced("gpt-oss-120b", 0.25, 0.69), unpriced("qwen-3.8-27b")],
    extra_headers: &[],
    // gpt-oss is a reasoning model; `low` is the closest equivalent of the
    // production path's `enable_thinking=false`.
    extra_body: r#"{"reasoning_effort":"low"}"#,
    stream_usage: true,
    accepts_chat_template_kwargs: false,
    key_name: "cerebras",
    // The one metered provider allowed: paid API terms do not train on
    // inputs, and the CostGuard windows bound the spend.
    billing: BillingClass::Metered,
    privacy_ok_for_private_memory: true,
    context_limit_tokens: Some(131_072),
    approx_tpm_limit: None,
    reasoning_suppression: true,
    skip_reason: None,
};

const GROQ: ProviderSpec = ProviderSpec {
    id: "groq",
    base_url: "https://api.groq.com/openai/v1",
    auth: AuthSource::KeyFile,
    models: &[
        priced("openai/gpt-oss-20b", 0.075, 0.30),
        priced("openai/gpt-oss-120b", 0.15, 0.60),
        unpriced("qwen/qwen3.8-27b"),
    ],
    extra_headers: &[],
    extra_body: r#"{"reasoning_effort":"low"}"#,
    stream_usage: true,
    accepts_chat_template_kwargs: false,
    key_name: "groq",
    billing: BillingClass::FreeTier,
    // Free-tier data terms are treated conservatively: bench and public
    // prompts only, never private memory.
    privacy_ok_for_private_memory: false,
    context_limit_tokens: Some(131_072),
    // Free-tier per-minute budget is finite; the gate keeps large prompts
    // off this provider rather than spending them into a 429.
    approx_tpm_limit: Some(15_000),
    reasoning_suppression: true,
    skip_reason: None,
};

const OPENROUTER: ProviderSpec = ProviderSpec {
    id: "openrouter",
    base_url: "https://openrouter.ai/api/v1",
    auth: AuthSource::KeyFile,
    // Free-tier candidates only: the routing policy permits metered spend
    // on Cerebras alone, so a FreeTier provider must never fall back to a
    // paid model. `select_models` additionally drops any candidate whose
    // live catalog price is nonzero, so a `:free` tag turned paid cannot
    // slip through.
    models: &[
        priced("qwen/qwen3.8-27b:free", 0.0, 0.0),
        priced("meta-llama/llama-3.3-70b-instruct:free", 0.0, 0.0),
        priced("deepseek/deepseek-chat-v3.1:free", 0.0, 0.0),
    ],
    extra_headers: &[],
    // Ask OpenRouter to return the billed cost on each response, and keep
    // upstream reasoning minimal so the comparison is about chat latency.
    extra_body: r#"{"usage":{"include":true},"reasoning":{"effort":"low"}}"#,
    stream_usage: true,
    accepts_chat_template_kwargs: false,
    key_name: "openrouter",
    // The declared candidates are `:free` variants; `:free` plans may log or
    // train on inputs, so private material stays off this provider.
    billing: BillingClass::FreeTier,
    privacy_ok_for_private_memory: false,
    context_limit_tokens: None,
    approx_tpm_limit: None,
    // Measured: the free upstream ignored `reasoning.effort=low`.
    reasoning_suppression: false,
    skip_reason: None,
};

const ORCAROUTER: ProviderSpec = ProviderSpec {
    id: "orcarouter",
    base_url: "https://api.orcarouter.ai/v1",
    auth: AuthSource::KeyFile,
    // Free upstreams for the direct-provider comparison; auto-routing is a
    // separate experiment row. Billed cost is reported back per request via
    // X-OrcaRouter-Include-Cost, so these list prices are only pre-flight
    // estimates.
    models: &[
        priced("z-ai/glm-5.3-flash-free", 0.0, 0.0),
        priced("deepseek/deepseek-v4-flash-free", 0.0, 0.0),
        priced("tencent/hy3-free", 0.0, 0.0),
    ],
    extra_headers: &[("X-OrcaRouter-Include-Cost", "true")],
    // OrcaRouter validates request fields against its own schema — extra
    // body fields it does not know come back as 400, so nothing extra.
    extra_body: "",
    stream_usage: true,
    accepts_chat_template_kwargs: false,
    key_name: "orcarouter",
    billing: BillingClass::FreeTier,
    privacy_ok_for_private_memory: false,
    context_limit_tokens: None,
    approx_tpm_limit: None,
    // Measured: upstreams keep thinking; there is no honored suppression
    // field on this gateway.
    reasoning_suppression: false,
    skip_reason: None,
};

/// `orcarouter/auto` — the router picks the upstream per request. Its billed
/// cost and served model come back on the response; the upstream's data
/// terms are unknown, so it is treated as metered and non-private.
pub const ORCAROUTER_AUTO_SPEC: ProviderSpec = ProviderSpec {
    id: "orcarouter-auto",
    base_url: "https://api.orcarouter.ai/v1",
    auth: AuthSource::KeyFile,
    models: &[unpriced(ORCAROUTER_AUTO_MODEL)],
    extra_headers: &[("X-OrcaRouter-Include-Cost", "true")],
    extra_body: "",
    stream_usage: true,
    accepts_chat_template_kwargs: false,
    key_name: "orcarouter",
    billing: BillingClass::Metered,
    privacy_ok_for_private_memory: false,
    context_limit_tokens: None,
    approx_tpm_limit: None,
    reasoning_suppression: false,
    skip_reason: Some("auto-routing experiment; run with --orca-auto"),
};

/// Google's OpenAI-compatible Gemini endpoint. The free tier exists for
/// flash-lite class models; its data terms allow model improvement on
/// free inputs, so private material is never sent.
const GEMINI: ProviderSpec = ProviderSpec {
    id: "gemini",
    base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
    auth: AuthSource::KeyFile,
    models: &[
        unpriced("gemini-3.1-flash-lite"),
        unpriced("gemini-2.5-flash-lite"),
    ],
    extra_headers: &[],
    extra_body: r#"{"reasoning_effort":"low"}"#,
    stream_usage: true,
    accepts_chat_template_kwargs: false,
    key_name: "gemini",
    billing: BillingClass::FreeTier,
    privacy_ok_for_private_memory: false,
    context_limit_tokens: Some(1_048_576),
    approx_tpm_limit: None,
    reasoning_suppression: true,
    skip_reason: None,
};

/// OpenAI proper: metered. Free-tier promotions that share data for model
/// improvement are not a production path for persona prompts, so only the
/// paid plan is declared.
const OPENAI: ProviderSpec = ProviderSpec {
    id: "openai",
    base_url: "https://api.openai.com/v1",
    auth: AuthSource::KeyFile,
    models: &[
        priced("gpt-5-nano", 0.05, 0.40),
        priced("gpt-5-mini", 0.25, 2.00),
    ],
    extra_headers: &[],
    extra_body: r#"{"reasoning_effort":"minimal"}"#,
    stream_usage: true,
    accepts_chat_template_kwargs: false,
    key_name: "openai",
    billing: BillingClass::Metered,
    privacy_ok_for_private_memory: true,
    context_limit_tokens: Some(400_000),
    approx_tpm_limit: None,
    reasoning_suppression: true,
    skip_reason: None,
};

/// `orcarouter/auto` — the router picks the upstream per request. Only used
/// by the dedicated `--orca-auto` experiment. Its routed model and billed
/// cost come back on the response (`X-OrcaRouter-Include-Cost` above).
pub const ORCAROUTER_AUTO_MODEL: &str = "orcarouter/auto";

/// All declared providers, in report order. Jev is deliberately absent: it
/// is the decision/routing probe, not a language organ.
pub fn provider_registry() -> Vec<ProviderSpec> {
    let mut hai = HAI;
    hai.auth = AuthSource::Env(HAI_API_KEY_ENV.to_owned());
    let mut llm_master = LLM_MASTER;
    llm_master.auth = AuthSource::Env("KAMIMUSUHI_NODE_TOKEN".to_owned());
    vec![
        hai,
        llm_master,
        GROQ,
        CEREBRAS,
        OPENROUTER,
        ORCAROUTER,
        ORCAROUTER_AUTO_SPEC,
        GEMINI,
        OPENAI,
    ]
}

/// Bearer tokens held in process memory only.
///
/// Values are read from operator-named files (or the environment for
/// env-sourced providers) at startup/call time, and this store is the only
/// place they exist. Nothing about a token is ever returned for display —
/// `has()` answers presence, `token_for()` hands the value straight to
/// header construction, and `Debug` lists provider names, never values.
#[derive(Default)]
pub struct KeyStore {
    tokens: BTreeMap<String, String>,
}

impl std::fmt::Debug for KeyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyStore")
            .field("providers", &self.tokens.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl KeyStore {
    /// Read `provider:path` pairs. A missing file or empty content is a
    /// load error naming the provider, never the value.
    pub fn load(pairs: &[(String, std::path::PathBuf)]) -> Result<Self, String> {
        let mut tokens = BTreeMap::new();
        for (provider, path) in pairs {
            let raw = std::fs::read_to_string(path).map_err(|source| {
                format!(
                    "{provider}: cannot read key file {}: {source}",
                    path.display()
                )
            })?;
            let token = raw.trim();
            if token.is_empty() {
                return Err(format!("{provider}: key file {} is empty", path.display()));
            }
            tokens.insert(provider.clone(), token.to_owned());
        }
        Ok(Self { tokens })
    }

    pub fn has(&self, provider: &str) -> bool {
        self.tokens.contains_key(provider)
    }

    /// The token for a provider, for callers that hand it straight to a
    /// request builder. The value must never be printed, logged or stored.
    pub fn token_for(&self, provider: &str) -> Option<&str> {
        self.tokens.get(provider).map(String::as_str)
    }

    /// Build the Authorization header for a provider without exposing the
    /// value: env-sourced credentials are read here, at call time, and
    /// file-sourced ones come from this store.
    fn bearer(&self, spec: &ProviderSpec) -> Result<Vec<Header>, BenchError> {
        let token = match &spec.auth {
            AuthSource::Env(name) => match std::env::var(name) {
                Ok(value) if !value.trim().is_empty() => value,
                _ => {
                    return Err(BenchError::NoCredential(format!(
                        "environment variable {name} is not set"
                    )));
                }
            },
            AuthSource::KeyFile => self
                .token_for(spec.key_name)
                .map(str::to_owned)
                .ok_or_else(|| {
                    BenchError::NoCredential(format!("no --key {}:<path> supplied", spec.key_name))
                })?,
            AuthSource::None => return Ok(Vec::new()),
        };
        Ok(vec![Header {
            name: "Authorization".to_owned(),
            value: format!("Bearer {}", token.trim()),
        }])
    }
}

/// How a benchmark call failed, in the same vocabulary the runtime uses.
/// Error text from the wire is never copied here — it can echo the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BenchError {
    NoCredential(String),
    InvalidConfig(String),
    Timeout,
    Transport,
    Tls(String),
    HttpStatus(u16),
    Malformed(String),
    RateLimited,
    Authentication,
    ProviderError(String),
}

impl BenchError {
    pub fn code(&self) -> String {
        match self {
            Self::NoCredential(_) => "NO_CREDENTIAL".to_owned(),
            Self::InvalidConfig(_) => "INVALID_CONFIG".to_owned(),
            Self::Timeout => "TIMEOUT".to_owned(),
            Self::Transport => "TRANSPORT".to_owned(),
            Self::Tls(_) => "TLS".to_owned(),
            Self::HttpStatus(status) => format!("HTTP_STATUS_{status}"),
            Self::Malformed(_) => "MALFORMED".to_owned(),
            Self::RateLimited => "RATE_LIMITED".to_owned(),
            Self::Authentication => "AUTHENTICATION".to_owned(),
            Self::ProviderError(code) => format!("PROVIDER_ERROR_{code}"),
        }
    }
}

impl std::fmt::Display for BenchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code())
    }
}

impl std::error::Error for BenchError {}

fn map_http(error: HttpError) -> BenchError {
    match error {
        HttpError::InvalidRequest(reason) => BenchError::InvalidConfig(reason),
        HttpError::Timeout { .. } => BenchError::Timeout,
        HttpError::Transport(_) => BenchError::Transport,
        HttpError::Malformed(detail) => BenchError::Malformed(detail),
        HttpError::Tls { kind, .. } => BenchError::Tls(kind.as_str().to_owned()),
    }
}

/// One model from a provider's `GET /models` catalog, with per-token USD
/// pricing when the catalog publishes it (OpenRouter does; others omit it).
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_usd_per_mtok: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_usd_per_mtok: Option<f64>,
}

fn catalog_price(entry: &serde_json::Value) -> (Option<f64>, Option<f64>) {
    let per_mtok = |key: &str| -> Option<f64> {
        entry
            .pointer(&format!("/pricing/{key}"))
            .and_then(serde_json::Value::as_str)
            .and_then(|raw| raw.parse::<f64>().ok())
            .map(|per_token| per_token * 1_000_000.0)
    };
    (per_mtok("prompt"), per_mtok("completion"))
}

/// Where a reported cost figure came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CostKind {
    /// The provider billed this request and said so on the response.
    Actual,
    /// Static price table × measured token counts.
    Estimate,
    /// The model's price is not known to this run.
    Unknown,
}

/// One measured request. Secret-free by construction: there is no field that
/// could carry a credential, a prompt or a response body — only ids,
/// timings, counts and classifications.
#[derive(Debug, Clone, Serialize)]
pub struct BenchSample {
    pub provider: String,
    pub model: String,
    /// The model that actually answered (differs under auto-routing).
    pub served_model: Option<String>,
    pub prompt_id: String,
    pub rep: u32,
    pub ok: bool,
    pub http_status: Option<u16>,
    pub streamed: bool,
    /// Response-head time (TTFB): connect+TLS+server queue before body.
    pub headers_ms: Option<u64>,
    /// First SSE event of any kind (role marker, reasoning, content) —
    /// network+queue latency separate from generation start.
    pub first_event_ms: Option<u64>,
    /// Time to the first SSE content delta — the latency a user feels.
    pub ttft_ms: Option<u64>,
    pub total_ms: u64,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// Decode throughput while content streamed: completion tokens over
    /// (total − first content delta). None when unavailable.
    pub decode_tps: Option<f64>,
    pub response_chars: Option<usize>,
    /// Characters streamed into `delta.reasoning`/`reasoning_content` —
    /// thinking a reasoning model did before answering. Zero for models
    /// that do not think.
    pub reasoning_chars: usize,
    /// Prompt tokens served from the provider's prefix cache
    /// (`usage.prompt_tokens_details.cached_tokens`), when reported.
    pub cached_tokens: Option<u64>,
    /// The provider's first request of this run — cold measures connect +
    /// auth + a cold upstream; warm rows benefit from any provider-side
    /// prefix cache.
    pub cold: bool,
    /// The route-gate reason this provider was tried under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_reason: Option<String>,
    pub cost_usd: Option<f64>,
    pub cost_kind: CostKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a probe learned without spending a token.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub provider: String,
    pub ok: bool,
    pub latency_ms: u64,
    /// The catalog the provider advertised (empty when listing failed or
    /// the provider has no catalog endpoint).
    pub catalog: Vec<CatalogEntry>,
    pub selected_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A client for one registered provider. All OpenAI-compatible providers
/// share this implementation — the spec supplies only what differs.
pub struct BenchClient<'a> {
    pub spec: &'a ProviderSpec,
    keys: &'a KeyStore,
    timeout: Duration,
}

impl<'a> BenchClient<'a> {
    pub fn new(spec: &'a ProviderSpec, keys: &'a KeyStore, timeout: Duration) -> Self {
        Self {
            spec,
            keys,
            timeout,
        }
    }

    fn headers(&self) -> Result<Vec<Header>, BenchError> {
        let mut headers = self.keys.bearer(self.spec)?;
        for (name, value) in self.spec.extra_headers {
            headers.push(Header {
                name: (*name).to_owned(),
                value: (*value).to_owned(),
            });
        }
        Ok(headers)
    }

    /// `GET /models`: the free health check — it proves DNS, TLS, auth and
    /// catalog in one call without spending a token.
    pub fn probe(&self) -> ProbeResult {
        let started = Instant::now();
        let latency_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let fail = |error: BenchError| ProbeResult {
            provider: self.spec.id.to_owned(),
            ok: false,
            latency_ms: latency_ms(),
            catalog: Vec::new(),
            selected_model: None,
            error: Some(error.code()),
        };
        if self.spec.base_url.is_empty() {
            return fail(BenchError::InvalidConfig("no base_url".to_owned()));
        }
        match self.catalog() {
            Ok(entries) => ProbeResult {
                provider: self.spec.id.to_owned(),
                ok: true,
                latency_ms: latency_ms(),
                selected_model: self.select_model(&entries).map(|(model, _)| model),
                catalog: entries,
                error: None,
            },
            Err(error) => fail(error),
        }
    }

    /// `GET /models` — the provider's catalog, including per-token pricing
    /// where the provider publishes it. Costs no tokens.
    pub fn catalog(&self) -> Result<Vec<CatalogEntry>, BenchError> {
        let headers = self.headers()?;
        let endpoint =
            Endpoint::parse(self.spec.base_url, "/models").map_err(BenchError::InvalidConfig)?;
        let response =
            get_json(&endpoint, &headers, self.timeout, &TrustAnchors::Webpki).map_err(map_http)?;
        if !response.is_success() {
            return Err(status_error(response.status));
        }
        let parsed: serde_json::Value = serde_json::from_str(&response.body)
            .map_err(|_| BenchError::Malformed("model list is not JSON".to_owned()))?;
        let data = parsed
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| BenchError::Malformed("model list has no data array".to_owned()))?;
        Ok(data
            .iter()
            .filter_map(|entry| {
                let id = entry.get("id")?.as_str()?.to_owned();
                let (input_usd_per_mtok, output_usd_per_mtok) = catalog_price(entry);
                Some(CatalogEntry {
                    id,
                    input_usd_per_mtok,
                    output_usd_per_mtok,
                })
            })
            .collect())
    }

    /// Preferred models the catalog advertises, cheapest-first, up to
    /// `limit`. With no catalog answer the declared candidates stand.
    /// Live catalog prices replace the static table entry when both exist,
    /// so a stale static price can never underestimate a real one.
    pub fn select_models(
        &self,
        catalog: &[CatalogEntry],
        limit: usize,
    ) -> Vec<(String, ModelSpec)> {
        let mut out = Vec::new();
        for candidate in self.spec.models {
            let listed = catalog.iter().find(|entry| entry.id == candidate.id);
            if catalog.is_empty() || listed.is_some() {
                let spec = match listed {
                    Some(entry)
                        if entry.input_usd_per_mtok.is_some()
                            || entry.output_usd_per_mtok.is_some() =>
                    {
                        ModelSpec {
                            id: candidate.id,
                            input_usd_per_mtok: entry.input_usd_per_mtok,
                            output_usd_per_mtok: entry.output_usd_per_mtok,
                        }
                    }
                    _ => *candidate,
                };
                // A FreeTier provider must never silently become metered:
                // a candidate whose live price is nonzero is not eligible.
                if self.spec.billing == BillingClass::FreeTier
                    && (spec.input_usd_per_mtok.unwrap_or(0.0) > 0.0
                        || spec.output_usd_per_mtok.unwrap_or(0.0) > 0.0)
                {
                    continue;
                }
                out.push((candidate.id.to_owned(), spec));
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }

    /// First preferred model the catalog advertises.
    pub fn select_model(&self, catalog: &[CatalogEntry]) -> Option<(String, ModelSpec)> {
        self.select_models(catalog, 1).into_iter().next()
    }

    /// Merge provider body extras into a request body. `model`, `messages`,
    /// `stream` and `stream_options` are the caller's and cannot be set here.
    fn apply_body_extras(&self, body: &mut serde_json::Value) {
        if self.spec.extra_body.is_empty() {
            return;
        }
        let (Some(map), Ok(extras)) = (
            body.as_object_mut(),
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
                self.spec.extra_body,
            ),
        ) else {
            return;
        };
        for (key, value) in extras {
            map.insert(key, value);
        }
    }

    /// One non-streaming chat completion — the fallback for endpoints that
    /// do not honour `stream`, and the measurement of choice when only
    /// total latency matters. `ttft_ms` stays `None`: without a stream there
    /// is no first token, only a whole reply.
    pub fn chat(
        &self,
        model: &str,
        mut body: serde_json::Value,
        prompt_id: &str,
        rep: u32,
        price: Option<(f64, f64)>,
        max_tokens: u32,
    ) -> BenchSample {
        let started = Instant::now();
        let total_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut sample = self.blank_sample(model, prompt_id, rep, false);
        let endpoint = match Endpoint::parse(self.spec.base_url, "/chat/completions") {
            Ok(endpoint) => endpoint,
            Err(reason) => {
                return self.failed(sample, BenchError::InvalidConfig(reason), started);
            }
        };
        let headers = match self.headers() {
            Ok(headers) => headers,
            Err(error) => return self.failed(sample, error, started),
        };
        self.apply_body_extras(&mut body);
        body["max_tokens"] = serde_json::json!(max_tokens);
        let wire = body.to_string();
        let response = match post_json(
            &endpoint,
            &wire,
            &headers,
            self.timeout,
            &TrustAnchors::Webpki,
        ) {
            Ok(response) => response,
            Err(error) => return self.failed(sample, map_http(error), started),
        };
        sample.http_status = Some(response.status);
        if !response.is_success() {
            return self.failed(sample, status_error(response.status), started);
        }
        let parsed: serde_json::Value = match serde_json::from_str(&response.body) {
            Ok(parsed) => parsed,
            Err(_) => {
                return self.failed(
                    sample,
                    BenchError::Malformed("response body is not JSON".to_owned()),
                    started,
                );
            }
        };
        if let Some(code) = kamimusuhi_resource_http::openai_response::error_code(&parsed) {
            return self.failed(sample, BenchError::ProviderError(code.to_owned()), started);
        }
        sample.served_model = parsed
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let content = parsed
            .pointer("/choices/0/message/content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if content.trim().is_empty() {
            return self.failed(
                sample,
                BenchError::Malformed("provider returned empty content".to_owned()),
                started,
            );
        }
        sample.response_chars = Some(content.chars().count());
        if let Some(usage) = parsed.get("usage") {
            fill_usage(&mut sample, usage);
        }
        self.apply_price(&mut sample, price);
        sample.total_ms = total_ms();
        sample
    }

    fn blank_sample(&self, model: &str, prompt_id: &str, rep: u32, streamed: bool) -> BenchSample {
        BenchSample {
            provider: self.spec.id.to_owned(),
            model: model.to_owned(),
            served_model: None,
            prompt_id: prompt_id.to_owned(),
            rep,
            ok: true,
            http_status: None,
            streamed,
            headers_ms: None,
            first_event_ms: None,
            ttft_ms: None,
            total_ms: 0,
            prompt_tokens: None,
            completion_tokens: None,
            decode_tps: None,
            response_chars: None,
            reasoning_chars: 0,
            cached_tokens: None,
            cold: false,
            route_reason: None,
            cost_usd: None,
            cost_kind: CostKind::Unknown,
            error: None,
        }
    }

    fn failed(&self, mut sample: BenchSample, error: BenchError, started: Instant) -> BenchSample {
        sample.ok = false;
        sample.total_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        sample.error = Some(error.code());
        sample
    }

    fn apply_price(&self, sample: &mut BenchSample, price: Option<(f64, f64)>) {
        if sample.cost_usd.is_none()
            && let (Some((input_price, output_price)), Some(in_tokens), Some(out_tokens)) =
                (price, sample.prompt_tokens, sample.completion_tokens)
        {
            sample.cost_usd =
                Some((in_tokens as f64 * input_price + out_tokens as f64 * output_price) / 1e6);
            sample.cost_kind = CostKind::Estimate;
        }
    }

    /// One streaming chat completion. `body` is the complete OpenAI request
    /// (messages already assembled); this adds `stream`/`stream_options`,
    /// provider extras, and records TTFB, first-content time, usage and —
    /// when the provider reports it — billed cost.
    pub fn chat_stream(
        &self,
        model: &str,
        mut body: serde_json::Value,
        prompt_id: &str,
        rep: u32,
        price: Option<(f64, f64)>,
        max_tokens: u32,
    ) -> BenchSample {
        let started = Instant::now();
        let total_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut sample = self.blank_sample(model, prompt_id, rep, true);

        let endpoint = match Endpoint::parse(self.spec.base_url, "/chat/completions") {
            Ok(endpoint) => endpoint,
            Err(reason) => return self.failed(sample, BenchError::InvalidConfig(reason), started),
        };
        let headers = match self.headers() {
            Ok(headers) => headers,
            Err(error) => return self.failed(sample, error, started),
        };
        self.apply_body_extras(&mut body);
        body["stream"] = serde_json::Value::Bool(true);
        body["max_tokens"] = serde_json::json!(max_tokens);
        if self.spec.stream_usage {
            body["stream_options"] = serde_json::json!({"include_usage": true});
        }
        let wire = body.to_string();

        // SSE consumer state. Chunks may split an event anywhere; complete
        // `data:` lines are peeled off as they arrive.
        let mut sse_buffer = Vec::new();
        let mut content = String::new();
        let mut reasoning_chars = 0usize;
        let mut saw_done = false;
        let mut usage: Option<serde_json::Value> = None;
        let mut served_model: Option<String> = None;
        let result = post_json_stream(
            &endpoint,
            &wire,
            &headers,
            self.timeout,
            &TrustAnchors::Webpki,
            &mut |bytes| {
                sse_buffer.extend_from_slice(bytes);
                // One `data:` line per OpenAI-style event; split on LF and
                // trim CR so a chunk can end anywhere inside the stream.
                while let Some(end) = sse_buffer.iter().position(|b| *b == b'\n') {
                    let mut line = sse_buffer.drain(..=end).collect::<Vec<u8>>();
                    while matches!(line.last(), Some(b'\n' | b'\r')) {
                        line.pop();
                    }
                    let Ok(line) = String::from_utf8(line) else {
                        continue;
                    };
                    let Some(data) = line.trim().strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data == "[DONE]" {
                        saw_done = true;
                        continue;
                    }
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };
                    if sample.first_event_ms.is_none() {
                        sample.first_event_ms =
                            Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
                    }
                    if served_model.is_none()
                        && let Some(model) = event.get("model").and_then(serde_json::Value::as_str)
                    {
                        served_model = Some(model.to_owned());
                    }
                    if let Some(u) = event.get("usage") {
                        usage = Some(u.clone());
                    }
                    if sample.ttft_ms.is_none()
                        && event
                            .pointer("/choices/0/delta/content")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|delta| !delta.is_empty())
                    {
                        sample.ttft_ms =
                            Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
                    }
                    if let Some(delta) = event
                        .pointer("/choices/0/delta/content")
                        .and_then(serde_json::Value::as_str)
                    {
                        content.push_str(delta);
                    }
                    // Reasoning shows up as `reasoning` (gpt-oss) or
                    // `reasoning_content` (GLM et al.); it is thinking, not
                    // the reply, so it is counted, not concatenated.
                    for key in ["reasoning", "reasoning_content"] {
                        if let Some(delta) = event
                            .pointer(&format!("/choices/0/delta/{key}"))
                            .and_then(serde_json::Value::as_str)
                        {
                            reasoning_chars += delta.chars().count();
                        }
                    }
                }
                !saw_done
            },
        );
        let response = match result {
            Ok(response) => response,
            Err(error) => return self.failed(sample, map_http(error), started),
        };
        sample.http_status = Some(response.status);
        sample.headers_ms = Some(response.headers_ms);
        // A streamed reply that never emitted a delta still has a
        // first-byte time; TTFT stays None in that case.
        if !response.is_success() {
            return self.failed(sample, status_error(response.status), started);
        }
        sample.served_model = served_model;
        sample.response_chars = Some(content.chars().count());
        sample.reasoning_chars = reasoning_chars;
        if let Some(usage) = &usage {
            fill_usage(&mut sample, usage);
        }
        self.apply_price(&mut sample, price);
        if let (Some(ttft), Some(out_tokens)) = (sample.ttft_ms, sample.completion_tokens)
            && let decode_ms = total_ms().saturating_sub(ttft)
            && decode_ms > 0
        {
            sample.decode_tps = Some(out_tokens as f64 * 1000.0 / decode_ms as f64);
        }
        if content.trim().is_empty() {
            return self.failed(
                sample,
                BenchError::Malformed("stream produced no content".to_owned()),
                started,
            );
        }
        sample.total_ms = total_ms();
        sample
    }
}

fn status_error(status: u16) -> BenchError {
    match status {
        401 | 403 => BenchError::Authentication,
        429 => BenchError::RateLimited,
        other => BenchError::HttpStatus(other),
    }
}

/// Fill token/cost fields from a `usage` object. OrcaRouter reports
/// `cost_usd`; OpenRouter reports `cost`; cached tokens ride
/// `prompt_tokens_details.cached_tokens`.
fn fill_usage(sample: &mut BenchSample, usage: &serde_json::Value) {
    sample.prompt_tokens = usage
        .get("prompt_tokens")
        .and_then(serde_json::Value::as_u64);
    sample.completion_tokens = usage
        .get("completion_tokens")
        .and_then(serde_json::Value::as_u64);
    sample.cached_tokens = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            usage
                .get("cached_tokens")
                .and_then(serde_json::Value::as_u64)
        });
    if let Some(cost) = usage
        .get("cost_usd")
        .or_else(|| usage.get("cost"))
        .and_then(serde_json::Value::as_f64)
    {
        sample.cost_usd = Some(cost);
        sample.cost_kind = CostKind::Actual;
    }
}

/// Cost windows, in USD. `request` bounds one call by its pre-flight
/// estimate; `session`, `daily` and `monthly` bound cumulative *metered*
/// spend. Free/subscription providers report zero cost, so only metered
/// rows move the counters.
#[derive(Debug, Clone, Copy)]
pub struct CostCaps {
    pub request_usd: f64,
    pub session_usd: f64,
    pub daily_usd: Option<f64>,
    pub monthly_usd: Option<f64>,
}

impl Default for CostCaps {
    fn default() -> Self {
        Self {
            request_usd: DEFAULT_REQUEST_COST_CAP_USD,
            session_usd: DEFAULT_TOTAL_COST_CAP_USD,
            daily_usd: None,
            monthly_usd: None,
        }
    }
}

/// Persistent spend ledger, so daily/monthly caps survive restarts. One
/// small JSON file; it stores dollar amounts keyed by date, nothing else —
/// no provider names needed to enforce a cap, and certainly no secrets.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CostLedger {
    /// "YYYY-MM-DD" -> USD spent that day (UTC).
    #[serde(default)]
    pub daily: BTreeMap<String, f64>,
    /// "YYYY-MM" -> USD spent that month (UTC).
    #[serde(default)]
    pub monthly: BTreeMap<String, f64>,
}

impl CostLedger {
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, body).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    pub fn record(&mut self, date: &str, usd: f64) {
        *self.daily.entry(date.to_owned()).or_default() += usd;
        *self.monthly.entry(date[..7].to_owned()).or_default() += usd;
    }

    pub fn day_total(&self, date: &str) -> f64 {
        self.daily.get(date).copied().unwrap_or(0.0)
    }

    pub fn month_total(&self, month: &str) -> f64 {
        self.monthly.get(month).copied().unwrap_or(0.0)
    }
}

/// Today's date as "YYYY-MM-DD" (UTC). Civil-from-days conversion; no clock
/// precision beyond the day is needed for a spend ledger.
pub fn today_utc() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    // Howard Hinnant's civil-from-days algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

/// What the run may spend, across four windows. Checked before every
/// request; the estimate uses the static/live price table and the
/// worst-case completion size, so a cap cannot be exceeded by a model
/// answering more than predicted.
#[derive(Debug)]
pub struct CostGuard {
    caps: CostCaps,
    spent_session_usd: f64,
    ledger: CostLedger,
    ledger_path: Option<std::path::PathBuf>,
    /// Requests whose price is unknown or unbilled. They cannot overrun a
    /// cap — but they also cannot prove they did not, so the count is
    /// reported.
    unpriced_requests: u32,
}

impl CostGuard {
    pub fn new(caps: CostCaps, ledger_path: Option<std::path::PathBuf>) -> Self {
        let ledger = ledger_path
            .as_ref()
            .map(|path| CostLedger::load(path))
            .unwrap_or_default();
        Self {
            caps,
            spent_session_usd: 0.0,
            ledger,
            ledger_path,
            unpriced_requests: 0,
        }
    }

    /// Worst-case cost of one request at list prices.
    pub fn estimate(
        &self,
        price: Option<(f64, f64)>,
        est_prompt_tokens: u64,
        max_completion_tokens: u32,
    ) -> Option<f64> {
        let (input_price, output_price) = price?;
        Some(
            (est_prompt_tokens as f64 * input_price
                + f64::from(max_completion_tokens) * output_price)
                / 1e6,
        )
    }

    /// Whether the request may be sent. Refuses when the estimate exceeds
    /// the per-request cap or any cumulative window is already at its cap.
    pub fn permit(&self, estimate_usd: Option<f64>) -> Result<(), String> {
        if let Some(estimate) = estimate_usd
            && estimate > self.caps.request_usd
        {
            return Err(format!(
                "estimated ${estimate:.4}/request exceeds the ${:.2} request cap",
                self.caps.request_usd
            ));
        }
        let estimate = estimate_usd.unwrap_or(0.0);
        if self.spent_session_usd + estimate > self.caps.session_usd {
            return Err(format!(
                "session cost cap ${:.2} reached",
                self.caps.session_usd
            ));
        }
        let today = today_utc();
        if let Some(cap) = self.caps.daily_usd
            && self.ledger.day_total(&today) + estimate > cap
        {
            return Err(format!("daily cost cap ${cap:.2} reached"));
        }
        if let Some(cap) = self.caps.monthly_usd
            && self.ledger.month_total(&today[..7]) + estimate > cap
        {
            return Err(format!("monthly cost cap ${cap:.2} reached"));
        }
        Ok(())
    }

    /// Account for a finished request: actual billed cost when reported,
    /// else the unpriced counter. Only real spend moves the windows.
    pub fn record(&mut self, sample: &BenchSample) {
        match sample.cost_usd {
            Some(cost) => self.record_cost(cost),
            None => self.unpriced_requests += 1,
        }
    }

    /// Record a known USD spend (metered actuals or estimates).
    pub fn record_cost(&mut self, cost_usd: f64) {
        if cost_usd <= 0.0 {
            return;
        }
        self.spent_session_usd += cost_usd;
        self.ledger.record(&today_utc(), cost_usd);
        if let Some(path) = &self.ledger_path {
            // Best-effort persistence: a failed save must not break a run.
            let _ = self.ledger.save(path);
        }
    }

    /// Count a request that reported no cost basis.
    pub fn record_unpriced(&mut self) {
        self.unpriced_requests += 1;
    }

    pub fn spent_usd(&self) -> f64 {
        self.spent_session_usd
    }

    pub fn day_spent_usd(&self) -> f64 {
        self.ledger.day_total(&today_utc())
    }

    pub fn month_spent_usd(&self) -> f64 {
        self.ledger.month_total(&today_utc()[..7])
    }

    pub fn unpriced_requests(&self) -> u32 {
        self.unpriced_requests
    }

    pub fn remaining_usd(&self) -> f64 {
        (self.caps.session_usd - self.spent_session_usd).max(0.0)
    }
}

/// The routing vocabulary Jev is asked to choose from. These are *classes
/// of work*, not provider names — the mapping from class to provider pool is
/// host-side policy and stays out of the wire request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RouteClass {
    /// A short conversational turn: answer fast and cheaply.
    FastChat,
    /// Content that should never leave the local node.
    LocalChat,
    /// Multi-step or high-stakes reasoning; latency is secondary.
    DeepReasoning,
    /// A turn that will need tool calls.
    ToolTask,
    /// A turn needing heavy recall/context assembly.
    MemoryHeavy,
}

impl RouteClass {
    pub const ALL: [Self; 5] = [
        Self::FastChat,
        Self::LocalChat,
        Self::DeepReasoning,
        Self::ToolTask,
        Self::MemoryHeavy,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FastChat => "FAST_CHAT",
            Self::LocalChat => "LOCAL_CHAT",
            Self::DeepReasoning => "DEEP_REASONING",
            Self::ToolTask => "TOOL_TASK",
            Self::MemoryHeavy => "MEMORY_HEAVY",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.as_str() == raw)
    }

    fn criterion(self) -> &'static str {
        match self {
            Self::FastChat => "Short casual conversation where low latency matters most.",
            Self::LocalChat => {
                "Content that should stay on the local node: private material, or a turn the local model can fully serve."
            }
            Self::DeepReasoning => {
                "Multi-step reasoning, analysis or a hard question where quality beats latency."
            }
            Self::ToolTask => "The turn requires tool calls or external lookups to answer.",
            Self::MemoryHeavy => {
                "The turn depends on recalling conversation history, stored memory or large context."
            }
        }
    }
}

/// A shadow routing decision: where Jev would have sent this turn, recorded
/// for later comparison — never acted on by the production path.
#[derive(Debug, Clone, Serialize)]
pub struct RouteDecision {
    pub route_class: String,
    pub confidence: f32,
    pub probabilities: BTreeMap<String, f32>,
    pub model: String,
    pub latency_ms: u64,
    /// Always true in this module: shadow classification never routes.
    pub shadow: bool,
}

/// Ask Jev's `/v1/systemone` which route class a user turn belongs to.
///
/// The wire shape matches `JevDecisionProvider`: `{model, state, questions}`
/// in, a validated typed choice out. `token` comes from the caller's secret
/// store and is used for this request only.
pub fn classify_route(
    config: &TypesafeConfig,
    token: &str,
    user_text: &str,
    core_state: &ConversationCoreState,
    timeout: Duration,
) -> Result<RouteDecision, BenchError> {
    let endpoint =
        Endpoint::parse(&config.base_url, "/v1/systemone").map_err(BenchError::InvalidConfig)?;
    let headers = vec![Header {
        name: "Authorization".to_owned(),
        value: format!("Bearer {}", token.trim()),
    }];
    let criteria: BTreeMap<String, String> = RouteClass::ALL
        .iter()
        .map(|class| (class.as_str().to_owned(), class.criterion().to_owned()))
        .collect();
    let state = serde_json::json!({
        "user_text": user_text,
        "speech_act": infer_speech_act(user_text),
        "state": core_state,
    })
    .to_string();
    let body = serde_json::json!({
        "model": config.model,
        "state": state,
        "questions": {
            "route_class": {
                "type": "choice",
                "instructions": "Classify this user turn into the route class that should serve it. Choose exactly one declared class.",
                "criteria": criteria,
            }
        },
    })
    .to_string();
    let started = Instant::now();
    let response =
        post_json(&endpoint, &body, &headers, timeout, &TrustAnchors::Webpki).map_err(map_http)?;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if !response.is_success() {
        return Err(status_error(response.status));
    }
    let parsed: serde_json::Value = serde_json::from_str(&response.body)
        .map_err(|_| BenchError::Malformed("route response is not JSON".to_owned()))?;
    let answer = parsed
        .get("answers")
        .and_then(|answers| answers.get("route_class"))
        .ok_or_else(|| BenchError::Malformed("missing route_class answer".to_owned()))?;
    if answer.get("type").and_then(serde_json::Value::as_str) != Some("choice") {
        return Err(BenchError::Malformed(
            "route_class answer is not a choice".to_owned(),
        ));
    }
    let choice = answer
        .get("choice")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| BenchError::Malformed("route_class choice is missing".to_owned()))?;
    let route_class = RouteClass::parse(choice)
        .ok_or_else(|| BenchError::Malformed("route_class is outside the vocabulary".to_owned()))?;
    let confidence = answer
        .get("confidence")
        .and_then(serde_json::Value::as_f64)
        .filter(|value| (0.0..=1.0).contains(value))
        .ok_or_else(|| BenchError::Malformed("route_class confidence is invalid".to_owned()))?;
    let mut probabilities = BTreeMap::new();
    if let Some(map) = answer
        .get("probabilities")
        .and_then(serde_json::Value::as_object)
    {
        for class in RouteClass::ALL {
            if let Some(value) = map.get(class.as_str()).and_then(serde_json::Value::as_f64)
                && (0.0..=1.0).contains(&value)
            {
                probabilities.insert(class.as_str().to_owned(), value as f32);
            }
        }
    }
    Ok(RouteDecision {
        route_class: route_class.as_str().to_owned(),
        confidence: confidence as f32,
        probabilities,
        model: config.model.clone(),
        latency_ms,
        shadow: true,
    })
}

/// Nearest-rank percentile of a sorted-or-unsorted sample set. `None` on an
/// empty set; with few samples the caller should mark p95 as indicative
/// only.
pub fn percentile(samples: &[u64], pct: f64) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = ((pct / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted
        .get(rank.saturating_sub(1).min(sorted.len() - 1))
        .copied()
}

/// Per-(provider, model) aggregate over its successful samples. Models are
/// never mixed into one row: a fallback model gets its own.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderSummary {
    pub provider: String,
    pub model: String,
    pub samples: u32,
    pub errors: u32,
    /// Mean time to the first SSE event (network+queue, before generation).
    pub mean_first_event_ms: Option<u64>,
    /// TTFT of the first request to this pair — connect+auth+cold upstream.
    pub cold_ttft_ms: Option<u64>,
    /// p50 TTFT over requests after the first — provider-side prefix caches
    /// and warm connections apply.
    pub warm_p50_ttft_ms: Option<u64>,
    pub p50_ttft_ms: Option<u64>,
    /// Reference only when `samples` is small — a p95 over 3-5 draws is a
    /// description of this run, not a service-level estimate.
    pub p95_ttft_ms: Option<u64>,
    pub p50_total_ms: Option<u64>,
    pub mean_decode_tps: Option<f64>,
    pub mean_reasoning_chars: Option<f64>,
    pub cost_usd_total: f64,
    pub cost_kind: CostKind,
}

/// Summarize every (provider, model) pair present in `samples`, in first-seen
/// order.
pub fn summarize_all(samples: &[BenchSample]) -> Vec<ProviderSummary> {
    let mut order: Vec<(String, String)> = Vec::new();
    for sample in samples {
        let key = (sample.provider.clone(), sample.model.clone());
        if !order.contains(&key) {
            order.push(key);
        }
    }
    order
        .iter()
        .filter_map(|(provider, model)| summarize(provider, model, samples))
        .collect()
}

/// Aggregate one (provider, model) pair.
pub fn summarize(provider: &str, model: &str, samples: &[BenchSample]) -> Option<ProviderSummary> {
    let own: Vec<&BenchSample> = samples
        .iter()
        .filter(|s| s.provider == provider && s.model == model)
        .collect();
    if own.is_empty() {
        return None;
    }
    let ok: Vec<&&BenchSample> = own.iter().filter(|s| s.ok).collect();
    let ttfts: Vec<u64> = ok.iter().filter_map(|s| s.ttft_ms).collect();
    let cold_ttft = own.iter().find(|s| s.cold).and_then(|s| s.ttft_ms);
    let warm_ttfts: Vec<u64> = own
        .iter()
        .filter(|s| s.ok && !s.cold)
        .filter_map(|s| s.ttft_ms)
        .collect();
    let totals: Vec<u64> = ok.iter().map(|s| s.total_ms).collect();
    let firsts: Vec<u64> = ok.iter().filter_map(|s| s.first_event_ms).collect();
    let tps: Vec<f64> = ok.iter().filter_map(|s| s.decode_tps).collect();
    let thinking: Vec<f64> = ok.iter().map(|s| s.reasoning_chars as f64).collect();
    let cost_total: f64 = own.iter().filter_map(|s| s.cost_usd).sum();
    let cost_kind = if own.iter().all(|s| s.cost_kind == CostKind::Actual) {
        CostKind::Actual
    } else if own.iter().any(|s| s.cost_kind == CostKind::Estimate) {
        CostKind::Estimate
    } else {
        CostKind::Unknown
    };
    Some(ProviderSummary {
        provider: provider.to_owned(),
        model: model.to_owned(),
        samples: u32::try_from(ok.len()).unwrap_or(u32::MAX),
        errors: u32::try_from(own.len() - ok.len()).unwrap_or(u32::MAX),
        mean_first_event_ms: (!firsts.is_empty())
            .then(|| firsts.iter().sum::<u64>() / firsts.len() as u64),
        cold_ttft_ms: cold_ttft,
        warm_p50_ttft_ms: percentile(&warm_ttfts, 50.0),
        p50_ttft_ms: percentile(&ttfts, 50.0),
        p95_ttft_ms: percentile(&ttfts, 95.0),
        p50_total_ms: percentile(&totals, 50.0),
        mean_decode_tps: (!tps.is_empty()).then(|| tps.iter().sum::<f64>() / tps.len() as f64),
        mean_reasoning_chars: (!thinking.is_empty())
            .then(|| thinking.iter().sum::<f64>() / thinking.len() as f64),
        cost_usd_total: cost_total,
        cost_kind,
    })
}

/// The whole run, serializable to a report file. Contains no credentials,
/// no prompt text beyond the fixed benchmark inputs, and no response bodies.
#[derive(Debug, Clone, Serialize)]
pub struct BenchReport {
    pub started_at: String,
    pub prompts: Vec<serde_json::Value>,
    pub prompt_style: String,
    pub max_completion_tokens: u32,
    pub probes: Vec<ProbeResult>,
    pub samples: Vec<BenchSample>,
    pub summaries: Vec<ProviderSummary>,
    /// Where Jev would have routed each prompt (shadow only).
    pub shadow_routes: Vec<serde_json::Value>,
    pub skipped: Vec<serde_json::Value>,
    /// Agent-lane CLIs found on PATH (presence only — nothing invoked).
    pub agent_lanes: Vec<serde_json::Value>,
    pub cost: serde_json::Value,
    pub notes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(provider: &str, ok: bool, ttft: Option<u64>, total: u64) -> BenchSample {
        BenchSample {
            provider: provider.to_owned(),
            model: "m".to_owned(),
            served_model: None,
            prompt_id: "p".to_owned(),
            rep: 0,
            ok,
            http_status: Some(200),
            streamed: true,
            headers_ms: ttft.map(|t| t - 1),
            first_event_ms: ttft.map(|t| t - 1),
            ttft_ms: ttft,
            total_ms: total,
            prompt_tokens: Some(100),
            completion_tokens: Some(40),
            decode_tps: None,
            response_chars: Some(20),
            reasoning_chars: 0,
            cached_tokens: None,
            cold: false,
            route_reason: None,
            cost_usd: None,
            cost_kind: CostKind::Unknown,
            error: None,
        }
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        assert_eq!(percentile(&[10, 20, 30, 40, 50], 50.0), Some(30));
        assert_eq!(percentile(&[10, 20, 30, 40, 50], 95.0), Some(50));
        assert_eq!(percentile(&[50], 95.0), Some(50));
        assert_eq!(percentile(&[], 50.0), None);
        // Unsorted input is still ranked correctly.
        assert_eq!(percentile(&[50, 10, 30], 50.0), Some(30));
    }

    fn caps(request: f64, session: f64) -> CostCaps {
        CostCaps {
            request_usd: request,
            session_usd: session,
            daily_usd: None,
            monthly_usd: None,
        }
    }

    #[test]
    fn guard_refuses_an_over_cap_estimate() {
        let guard = CostGuard::new(caps(0.02, 0.50), None);
        // $1.00/Mtok in and out, 1000 est prompt tokens, 160 max completion.
        let estimate = guard.estimate(Some((1.0, 1.0)), 1000, 160);
        assert_eq!(estimate, Some(0.001_16));
        assert!(guard.permit(estimate).is_ok());

        let expensive = guard.estimate(Some((100.0, 100.0)), 1000, 160);
        assert!(guard.permit(expensive).is_err());
        // Unknown price is permitted but counted as unpriced.
        assert!(guard.permit(None).is_ok());
    }

    #[test]
    fn guard_stops_at_the_total_cap() {
        let mut guard = CostGuard::new(caps(0.02, 0.50), None);
        let mut s = sample("p", true, Some(10), 20);
        s.cost_usd = Some(0.49);
        s.cost_kind = CostKind::Actual;
        guard.record(&s);
        assert!(guard.permit(Some(0.001)).is_ok());
        s.cost_usd = Some(0.02);
        guard.record(&s);
        assert!(guard.permit(Some(0.001)).is_err());
    }

    #[test]
    fn guard_daily_and_monthly_windows() {
        let mut guard = CostGuard::new(
            CostCaps {
                request_usd: 0.02,
                session_usd: 0.50,
                daily_usd: Some(0.10),
                monthly_usd: Some(2.00),
            },
            None,
        );
        // Session is fresh; windows move with recorded cost.
        guard.record_cost(0.08);
        assert!(guard.permit(Some(0.015)).is_ok());
        guard.record_cost(0.015);
        // Day total is 0.095; another 0.01 would still fit, 0.02 would not.
        assert!(guard.permit(Some(0.004)).is_ok());
        assert!(guard.permit(Some(0.02)).is_err());
        assert!((guard.day_spent_usd() - 0.095).abs() < 1e-9);
        assert!((guard.month_spent_usd() - 0.095).abs() < 1e-9);
        // A zero/negative spend never moves the ledger.
        guard.record_cost(0.0);
        assert!((guard.day_spent_usd() - 0.095).abs() < 1e-9);
    }

    #[test]
    fn ledger_persists_and_keys_by_date() {
        let dir = std::env::temp_dir().join("provider-bench-ledger-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("ledger.json");
        let mut ledger = CostLedger::default();
        ledger.record("2026-09-23", 0.04);
        ledger.record("2026-09-23", 0.01);
        ledger.record("2026-09-24", 0.02);
        ledger.save(&path).expect("save");
        let loaded = CostLedger::load(&path);
        assert!((loaded.day_total("2026-09-23") - 0.05).abs() < 1e-9);
        assert!((loaded.day_total("2026-09-24") - 0.02).abs() < 1e-9);
        assert!((loaded.month_total("2026-09") - 0.07).abs() < 1e-9);
        assert_eq!(loaded.day_total("1999-01-01"), 0.0);
    }

    #[test]
    fn sized_prompt_grows_with_target() {
        let spec = &provider_registry()[0];
        let small = prompt_body_sized(spec, "test-model", "こんばんは", 2_000);
        let large = prompt_body_sized(spec, "test-model", "こんばんは", 16_000);
        let small_len = small.to_string().len();
        let large_len = large.to_string().len();
        assert!(large_len > small_len * 3);
        // The padding is conversation history, so the wire shape is intact.
        assert_eq!(large["model"], "test-model");
        assert!(large["messages"].as_array().unwrap().len() > 4);
        // A target at or below the base persona size adds no padding.
        let base = prompt_body_sized(spec, "test-model", "hi", 200);
        assert_eq!(base["model"], "test-model");
        assert!(
            base["messages"].as_array().unwrap().len()
                < large["messages"].as_array().unwrap().len()
        );
    }

    #[test]
    fn route_class_vocabulary_is_complete() {
        assert_eq!(RouteClass::ALL.len(), 5);
        for class in RouteClass::ALL {
            assert_eq!(RouteClass::parse(class.as_str()), Some(class));
        }
        assert_eq!(RouteClass::parse("SOMETHING_ELSE"), None);
    }

    #[test]
    fn registry_carries_no_credentials() {
        for spec in provider_registry() {
            match &spec.auth {
                // Env names and file markers only — never a value.
                AuthSource::Env(name) => {
                    assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
                }
                AuthSource::KeyFile | AuthSource::None => {}
            }
            assert!(spec.base_url.is_empty() || spec.base_url.starts_with("https://"));
            for (name, _) in spec.extra_headers {
                assert!(!name.eq_ignore_ascii_case("authorization"));
            }
        }
        // llm_master is declared but must never be probed overnight.
        let master = provider_registry()
            .into_iter()
            .find(|s| s.id == "llm_master")
            .expect("llm_master is registered");
        assert!(master.skip_reason.is_some());
    }

    #[test]
    fn catalog_price_reads_openrouter_pricing() {
        let entry = serde_json::json!({
            "id": "vendor/model",
            "pricing": {"prompt": "0.00000005", "completion": "0.00000010"}
        });
        let (input, output) = catalog_price(&entry);
        assert!((input.unwrap_or(0.0) - 0.05).abs() < 1e-9);
        assert!((output.unwrap_or(0.0) - 0.10).abs() < 1e-9);
        let (none_in, none_out) = catalog_price(&serde_json::json!({"id": "x"}));
        assert_eq!(none_in, None);
        assert_eq!(none_out, None);
    }

    #[test]
    fn summarize_aggregates_one_provider_model() {
        let samples = vec![
            sample("a", true, Some(100), 500),
            sample("a", true, Some(120), 600),
            sample("a", false, None, 30),
            sample("b", true, Some(10), 100),
        ];
        let summary = summarize("a", "m", &samples).expect("summary");
        assert_eq!(summary.samples, 2);
        assert_eq!(summary.errors, 1);
        assert_eq!(summary.p50_ttft_ms, Some(100));
        assert_eq!(summary.p95_ttft_ms, Some(120));
        assert_eq!(summary.p50_total_ms, Some(500));
        assert!(summarize("missing", "m", &samples).is_none());
        // summarize_all groups every (provider, model) pair seen.
        let all = summarize_all(&samples);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].provider, "a");
        assert_eq!(all[1].provider, "b");
    }

    #[test]
    fn prompt_body_bare_matches_openai_shape() {
        let spec = &provider_registry()[0];
        let body = prompt_body(spec, "test-model", "こんばんは", false);
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "こんばんは");
    }

    #[test]
    fn prompt_body_persona_includes_system_and_context() {
        let spec = &provider_registry()[0];
        let body = prompt_body(spec, "test-model", "こんばんは", true);
        assert_eq!(body["model"], "test-model");
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages[0]["role"], "system");
        // The system prompt carries the persona sections, not a bare greeting.
        let system = messages[0]["content"].as_str().unwrap_or_default();
        assert!(system.len() > 200);
        let user = messages.last().expect("user message");
        assert_eq!(user["role"], "user");
        assert!(
            user["content"]
                .as_str()
                .unwrap_or_default()
                .contains("こんばんは")
        );
    }

    #[test]
    fn free_tier_never_selects_a_priced_model() {
        // A FreeTier provider whose catalog prices a candidate above zero
        // must not serve it — the only metered path in this system is the
        // explicitly-guarded paid fallback.
        let mut spec = provider_registry()
            .into_iter()
            .find(|s| s.id == "openrouter")
            .expect("openrouter is registered");
        spec.models = Box::leak(Box::new([priced("vendor/paid-model", 0.5, 0.5)]));
        spec.billing = BillingClass::FreeTier;
        let keys = KeyStore::default();
        let client = BenchClient::new(&spec, &keys, Duration::from_secs(1));
        assert!(client.select_models(&[], 3).is_empty());
        // And the declared OpenRouter list really is free-only.
        let declared = provider_registry()
            .into_iter()
            .find(|s| s.id == "openrouter")
            .expect("openrouter is registered");
        for model in declared.models {
            assert_eq!(model.input_usd_per_mtok, Some(0.0));
            assert_eq!(model.output_usd_per_mtok, Some(0.0));
        }
    }

    #[test]
    fn keystore_loads_without_echoing_values() {
        let dir = std::env::temp_dir().join("provider-bench-keystore-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("key-a");
        std::fs::write(&path, "  test-token-value  \n").expect("write");
        let keys = KeyStore::load(&[("a".to_owned(), path)]).expect("load");
        assert!(keys.has("a"));
        assert_eq!(keys.token_for("a"), Some("test-token-value"));
        // Debug output names providers but must never contain a token value.
        let debug = format!("{keys:?}");
        assert!(debug.contains('a'));
        assert!(!debug.contains("test-token-value"));
        assert!(keys.token_for("b").is_none());
    }
}
