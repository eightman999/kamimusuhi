//! Compare chat latency across configured providers, routed by the gate.
//!
//! ```text
//! provider-bench --key groq:/path/to/key --key cerebras:/path ...
//! ```
//!
//! What this does, in order:
//!
//! 1. Loads bearer tokens from operator-named key files (never prints or
//!    stores them; env-sourced providers keep their env convention).
//! 2. Probes each selected provider with `GET /models` — free, and it
//!    validates auth and the catalog before a single token is spent.
//! 3. Asks Jev which route class each prompt belongs to, on a background
//!    thread — the gate never serializes Jev ahead of generation.
//! 4. Runs prompt units through the streaming chat path in gate order.
//!    `--prompt-tokens 2000,8000,16000,24000,28000` measures
//!    production-scale prompts; otherwise the three fixed Mio prompts run.
//!    The [`RouteGate`] decides eligibility (billing class, context/TPM,
//!    privacy, 429 cooldowns, failure rate) and each sample records its
//!    route reason.
//! 5. Optionally runs the same prompts through `orcarouter/auto` so the
//!    router's upstream choice can be compared against the gate's.
//! 6. Writes a secret-free JSON report and persists the cost ledger.
//!
//! The benchmark is deliberately cheap: free-tier first, metered last, hard
//! cost caps at request/session/daily/monthly windows. `llm_master` is
//! declared in the registry but never contacted — the node is off-duty.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use kamimusuhi_runtime::llm_jev::{
    ConversationCoreState, DEFAULT_TYPESAFE_BASE_URL, DEFAULT_TYPESAFE_MODEL, TYPESAFE_API_KEY_ENV,
    TYPESAFE_BASE_URL_ENV, TYPESAFE_MODEL_ENV, TypesafeConfig,
};
use kamimusuhi_runtime::provider_bench::{
    AuthSource, BENCH_PROMPTS, BenchClient, BenchReport, CostCaps, CostGuard, KeyStore,
    ORCAROUTER_AUTO_SPEC, ProbeResult, ProviderSpec, classify_route, prompt_body,
    prompt_body_sized, provider_registry, summarize_all,
};
use kamimusuhi_runtime::route_gate::{
    ProviderStateBook, RouteGate, RouteLane, RouteReason, probe_agent_lanes,
};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_REPS: u32 = 3;
const DEFAULT_LEDGER: &str = ".local/provider-bench/ledger.json";

#[derive(Debug)]
struct Options {
    keys: Vec<(String, PathBuf)>,
    providers: Option<Vec<String>>,
    reps: u32,
    persona_style: bool,
    orca_auto: bool,
    max_tokens: u32,
    timeout_ms: u64,
    request_cap_usd: f64,
    total_cap_usd: f64,
    daily_cap_usd: Option<f64>,
    monthly_cap_usd: Option<f64>,
    prompt_token_sizes: Vec<u64>,
    private: bool,
    ledger: PathBuf,
    out: Option<PathBuf>,
    list_models: bool,
    dry_run: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            providers: None,
            reps: DEFAULT_REPS,
            persona_style: true,
            orca_auto: false,
            max_tokens: kamimusuhi_runtime::provider_bench::DEFAULT_MAX_COMPLETION_TOKENS,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            request_cap_usd: kamimusuhi_runtime::provider_bench::DEFAULT_REQUEST_COST_CAP_USD,
            total_cap_usd: kamimusuhi_runtime::provider_bench::DEFAULT_TOTAL_COST_CAP_USD,
            daily_cap_usd: None,
            monthly_cap_usd: None,
            prompt_token_sizes: Vec::new(),
            private: false,
            ledger: PathBuf::from(DEFAULT_LEDGER),
            out: None,
            list_models: false,
            dry_run: false,
        }
    }
}

fn usage() -> &'static str {
    "Usage: provider-bench [OPTIONS]\n\
     \n\
     Credentials (repeatable, values never printed or stored):\n\
     \x20 --key <provider>:<path>      provider id : local key file\n\
     \n\
     Selection:\n\
     \x20 --providers <csv>            e.g. groq,cerebras,openrouter (default: all ready)\n\
     \x20 --orca-auto                  extra phase: same prompts via orcarouter/auto\n\
     \x20 --prompt-style persona|bare  default persona (real prompt assembly)\n\
     \x20 --prompt-tokens <csv>        sized prompts, e.g. 2000,8000,16000,24000,28000\n\
     \x20 --private                    mark prompts private (free-tier providers excluded)\n\
     \n\
     Cost guards:\n\
     \x20 --max-request-cost-usd F     per-request estimate cap (default 0.02)\n\
     \x20 --max-total-cost-usd F       session cap (default 0.50)\n\
     \x20 --max-daily-cost-usd F       cumulative daily cap (ledger-persisted)\n\
     \x20 --max-monthly-cost-usd F     cumulative monthly cap (ledger-persisted)\n\
     \x20 --ledger PATH                spend ledger (default .local/provider-bench/ledger.json)\n\
     \x20 --reps N                     samples per provider×prompt (default 3)\n\
     \x20 --max-tokens N               completion cap per request (default 768)\n\
     \x20 --timeout-ms N               request timeout (default 30000)\n\
     \n\
     Output:\n\
     \x20 --out PATH                   report path (default .local/provider-bench/)\n\
     \x20 --list-models                probe catalogs only, no generation\n\
     \x20 --dry-run                    plan + cost estimates only, no requests"
}

fn parse_args() -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut take = |flag: &str| -> Result<String, String> {
            args.next().ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--key" => {
                let spec = take("--key")?;
                let (provider, path) = spec
                    .split_once(':')
                    .ok_or_else(|| "--key takes <provider>:<path>".to_owned())?;
                if provider.is_empty() || path.is_empty() {
                    return Err("--key takes <provider>:<path>".to_owned());
                }
                options
                    .keys
                    .push((provider.to_owned(), PathBuf::from(path)));
            }
            "--providers" => {
                options.providers = Some(
                    take("--providers")?
                        .split(',')
                        .map(|id| id.trim().to_owned())
                        .filter(|id| !id.is_empty())
                        .collect(),
                );
            }
            "--reps" => {
                options.reps = take("--reps")?
                    .parse()
                    .map_err(|_| "--reps takes an integer".to_owned())?;
            }
            "--prompt-style" => {
                options.persona_style = match take("--prompt-style")?.as_str() {
                    "persona" => true,
                    "bare" => false,
                    other => return Err(format!("unknown prompt style {other}")),
                };
            }
            "--prompt-tokens" => {
                options.prompt_token_sizes = take("--prompt-tokens")?
                    .split(',')
                    .map(|s| {
                        s.trim()
                            .parse::<u64>()
                            .map_err(|_| "--prompt-tokens takes integers".to_owned())
                    })
                    .collect::<Result<_, _>>()?;
            }
            "--private" => options.private = true,
            "--orca-auto" => options.orca_auto = true,
            "--max-tokens" => {
                options.max_tokens = take("--max-tokens")?
                    .parse()
                    .map_err(|_| "--max-tokens takes an integer".to_owned())?;
            }
            "--timeout-ms" => {
                options.timeout_ms = take("--timeout-ms")?
                    .parse()
                    .map_err(|_| "--timeout-ms takes an integer".to_owned())?;
            }
            "--max-request-cost-usd" => {
                options.request_cap_usd = take("--max-request-cost-usd")?
                    .parse()
                    .map_err(|_| "--max-request-cost-usd takes a number".to_owned())?;
            }
            "--max-total-cost-usd" => {
                options.total_cap_usd = take("--max-total-cost-usd")?
                    .parse()
                    .map_err(|_| "--max-total-cost-usd takes a number".to_owned())?;
            }
            "--max-daily-cost-usd" => {
                options.daily_cap_usd = Some(
                    take("--max-daily-cost-usd")?
                        .parse()
                        .map_err(|_| "--max-daily-cost-usd takes a number".to_owned())?,
                );
            }
            "--max-monthly-cost-usd" => {
                options.monthly_cap_usd = Some(
                    take("--max-monthly-cost-usd")?
                        .parse()
                        .map_err(|_| "--max-monthly-cost-usd takes a number".to_owned())?,
                );
            }
            "--ledger" => options.ledger = PathBuf::from(take("--ledger")?),
            "--out" => options.out = Some(PathBuf::from(take("--out")?)),
            "--list-models" => options.list_models = true,
            "--dry-run" => options.dry_run = true,
            "--help" | "-h" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

/// Whether a provider can authenticate right now — env var set, or a
/// `--key` pair supplied. No credential value is touched here.
fn auth_ready(spec: &ProviderSpec, keys: &KeyStore) -> bool {
    match &spec.auth {
        AuthSource::Env(name) => std::env::var(name).is_ok_and(|v| !v.trim().is_empty()),
        AuthSource::KeyFile => keys.has(spec.key_name),
        AuthSource::None => true,
    }
}

fn auth_hint(spec: &ProviderSpec) -> String {
    match &spec.auth {
        AuthSource::Env(name) => format!("set {name}"),
        AuthSource::KeyFile => format!("pass --key {}:<path>", spec.key_name),
        AuthSource::None => "no credential needed".to_owned(),
    }
}

/// Estimate prompt tokens from the assembled request body. Conservative on
/// purpose: ~3 UTF-8 bytes per token is a high estimate for Japanese text,
/// and an over-estimate can only refuse a request, never overspend.
fn est_prompt_tokens(body: &serde_json::Value) -> u64 {
    (body.to_string().len() / 3).max(64) as u64
}

/// The Jev bearer token: file-supplied key first, then the standard env
/// var. The value goes straight into a request header; it is never printed.
fn jev_token(keys: &KeyStore) -> Option<String> {
    keys.token_for("jev").map(str::to_owned).or_else(|| {
        std::env::var(TYPESAFE_API_KEY_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
    })
}

/// One prompt unit: id, user text, and an optional target prompt size.
struct PromptUnit {
    id: String,
    text: String,
    target_tokens: Option<u64>,
}

fn prompt_units(options: &Options) -> Vec<PromptUnit> {
    if options.prompt_token_sizes.is_empty() {
        BENCH_PROMPTS
            .iter()
            .map(|(id, text)| PromptUnit {
                id: (*id).to_owned(),
                text: (*text).to_owned(),
                target_tokens: None,
            })
            .collect()
    } else {
        // One representative input per size; the padding carries the load.
        options
            .prompt_token_sizes
            .iter()
            .map(|size| PromptUnit {
                id: format!("size_{}k", size / 1000),
                text: "今日も終わるからご挨拶をと思ってね".to_owned(),
                target_tokens: Some(*size),
            })
            .collect()
    }
}

/// A provider cleared for use: spec, ordered model candidates, client.
struct Ready<'a> {
    spec: &'a ProviderSpec,
    models: Vec<(String, Option<(f64, f64)>)>,
    client: BenchClient<'a>,
}

fn reason_str(reason: &RouteReason) -> &'static str {
    match reason {
        RouteReason::Primary => "primary",
        RouteReason::BillingFallback => "billing_fallback",
        RouteReason::ContextFallback => "context_fallback",
        RouteReason::PrivacyFallback => "privacy_fallback",
        RouteReason::HealthFallback => "health_fallback",
    }
}

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n\n{}", usage());
            return ExitCode::from(2);
        }
    };
    let keys = match KeyStore::load(&options.keys) {
        Ok(keys) => keys,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let timeout = Duration::from_millis(options.timeout_ms);
    let gate = RouteGate::default();
    let mut health = ProviderStateBook::default();
    let mut notes: Vec<String> = Vec::new();
    let mut skipped: Vec<serde_json::Value> = Vec::new();

    // --- provider selection -------------------------------------------------
    let registry = provider_registry();
    let mut selected: Vec<&ProviderSpec> = Vec::new();
    for spec in &registry {
        let chosen = options
            .providers
            .as_ref()
            .is_none_or(|ids| ids.iter().any(|id| id == spec.id));
        if !chosen {
            continue;
        }
        if let Some(reason) = spec.skip_reason {
            skipped.push(serde_json::json!({
                "provider": spec.id, "stage": "selection", "reason": reason,
            }));
            notes.push(format!("{}: skipped — {}", spec.id, reason));
            continue;
        }
        if !auth_ready(spec, &keys) {
            skipped.push(serde_json::json!({
                "provider": spec.id, "stage": "selection",
                "reason": format!("no credential ({})", auth_hint(spec)),
            }));
            continue;
        }
        if options.private && !spec.privacy_ok_for_private_memory {
            skipped.push(serde_json::json!({
                "provider": spec.id, "stage": "selection",
                "reason": "private prompt; provider plan may train on inputs",
            }));
            continue;
        }
        selected.push(spec);
    }
    if selected.is_empty() {
        eprintln!("no provider is ready; supply --key pairs or credentials");
        return ExitCode::from(2);
    }

    // --- agent lanes: probe only ---------------------------------------------
    let agent_lanes = probe_agent_lanes();
    for lane in &agent_lanes {
        if lane.available {
            notes.push(format!(
                "{}: `{}` present (headless: `{}`); not invoked in this run",
                lane.lane, lane.cli, lane.headless_mode
            ));
        }
    }

    // --- probes: GET /models is free ----------------------------------------
    let mut probes: Vec<ProbeResult> = Vec::new();
    let mut ready: Vec<Ready<'_>> = Vec::new();
    for spec in &selected {
        let client = BenchClient::new(spec, &keys, timeout);
        let probe = client.probe();
        if probe.ok {
            let models = client
                .select_models(&probe.catalog, 2)
                .into_iter()
                .map(|(model, spec)| (model, spec.input_usd_per_mtok.zip(spec.output_usd_per_mtok)))
                .collect::<Vec<_>>();
            if !models.is_empty() {
                ready.push(Ready {
                    spec,
                    models,
                    client,
                });
            }
        }
        probes.push(probe);
    }
    for probe in &probes {
        if !probe.ok {
            skipped.push(serde_json::json!({
                "provider": probe.provider, "stage": "probe",
                "reason": probe.error.clone().unwrap_or_default(),
            }));
        }
    }
    if options.list_models {
        for probe in &probes {
            if probe.ok {
                let ids: Vec<&str> = probe.catalog.iter().map(|e| e.id.as_str()).collect();
                println!(
                    "{} ({}ms): {}",
                    probe.provider,
                    probe.latency_ms,
                    ids.join(", ")
                );
            } else {
                println!(
                    "{}: probe failed ({})",
                    probe.provider,
                    probe.error.clone().unwrap_or_default()
                );
            }
        }
        return ExitCode::SUCCESS;
    }
    let units = prompt_units(&options);

    // --- Jev shadow routing, in parallel -------------------------------------
    // Classification is spawned, not awaited before generation: Jev's
    // ~0.6s must never sit on the fast-chat critical path. The handle joins
    // before the report is written.
    let jev_handle = jev_token(&keys).map(|token| {
        let timeout = options.timeout_ms;
        std::thread::spawn(move || {
            let config = TypesafeConfig {
                base_url: std::env::var(TYPESAFE_BASE_URL_ENV)
                    .unwrap_or_else(|_| DEFAULT_TYPESAFE_BASE_URL.to_owned()),
                model: std::env::var(TYPESAFE_MODEL_ENV)
                    .unwrap_or_else(|_| DEFAULT_TYPESAFE_MODEL.to_owned()),
                auth_env: TYPESAFE_API_KEY_ENV.to_owned(),
                timeout_ms: timeout,
            };
            let mut out = Vec::new();
            for (prompt_id, text) in BENCH_PROMPTS {
                let core =
                    ConversationCoreState::for_input(&ConversationCoreState::default(), 0, text);
                match classify_route(&config, &token, text, &core, Duration::from_millis(timeout)) {
                    Ok(decision) => out.push(serde_json::json!({
                        "prompt_id": prompt_id,
                        "route_class": decision.route_class,
                        "confidence": decision.confidence,
                        "probabilities": decision.probabilities,
                        "latency_ms": decision.latency_ms,
                        "model": decision.model,
                        "shadow": decision.shadow,
                    })),
                    Err(error) => out.push(serde_json::json!({
                        "prompt_id": prompt_id,
                        "error": error.code(),
                        "shadow": true,
                    })),
                }
            }
            out
        })
    });
    if jev_handle.is_none() {
        notes.push(
            "jev: no credential (pass --key jev:<path> or set TYPESAFE_API_KEY); \
             shadow routing skipped"
                .to_owned(),
        );
    }

    // --- benchmark ------------------------------------------------------------
    let mut guard = CostGuard::new(
        CostCaps {
            request_usd: options.request_cap_usd,
            session_usd: options.total_cap_usd,
            daily_usd: options.daily_cap_usd,
            monthly_usd: options.monthly_cap_usd,
        },
        Some(options.ledger.clone()),
    );
    let mut samples = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();

    if options.dry_run {
        for unit in &units {
            for plan in &ready {
                for (model, price) in &plan.models {
                    let body = match unit.target_tokens {
                        Some(target) => prompt_body_sized(plan.spec, model, &unit.text, target),
                        None => prompt_body(plan.spec, model, &unit.text, options.persona_style),
                    };
                    let est = est_prompt_tokens(&body);
                    let estimate = guard.estimate(*price, est, options.max_tokens);
                    println!(
                        "  {} {} [{}] ~{} prompt tokens, est ${:.5}/request{}",
                        plan.spec.id,
                        model,
                        unit.id,
                        est,
                        estimate.unwrap_or(-1.0),
                        if estimate.is_none() {
                            " (price unknown)"
                        } else {
                            ""
                        }
                    );
                }
            }
        }
        return ExitCode::SUCCESS;
    }

    // Owned copies for the gate — it borrows the slice for the lifetime of
    // its candidate list.
    let owned_specs: Vec<ProviderSpec> = ready.iter().map(|p| p.spec.clone()).collect();
    let mut halt_reason: Option<String> = None;
    'outer: for unit in &units {
        // Estimate the prompt size once from the first provider's body —
        // bodies differ only by the model field.
        let (first_spec, first_model) = match ready
            .first()
            .and_then(|p| p.models.first().map(|(m, _)| (p.spec, m.as_str())))
        {
            Some(pair) => pair,
            None => break,
        };
        let est_tokens = match unit.target_tokens {
            Some(target) => target,
            None => est_prompt_tokens(&prompt_body(
                first_spec,
                first_model,
                &unit.text,
                options.persona_style,
            )),
        };
        // Gate order: cheapest eligible first, then health, then measured
        // latency. Ineligible providers are recorded, not silently dropped.
        let candidates = gate.select(
            RouteLane::FastChat,
            &owned_specs,
            est_tokens,
            options.private,
            &health,
            Instant::now(),
        );
        let eligible: BTreeSet<&str> = candidates.iter().map(|c| c.spec.id).collect();
        for plan in &ready {
            if !eligible.contains(plan.spec.id) {
                skipped.push(serde_json::json!({
                    "provider": plan.spec.id, "stage": "route_gate",
                    "prompt_id": unit.id,
                    "reason": "not eligible for this prompt (privacy/context/health)",
                }));
            }
        }
        // Run in gate order.
        let ordered: Vec<&Ready> = candidates
            .iter()
            .filter_map(|c| ready.iter().find(|p| p.spec.id == c.spec.id))
            .collect();
        let reason_by_id: std::collections::BTreeMap<&str, &RouteReason> =
            candidates.iter().map(|c| (c.spec.id, &c.reason)).collect();
        for plan in ordered {
            let mut model_idx = 0usize;
            let mut rep = 0;
            while rep < options.reps {
                let (model, price) = &plan.models[model_idx];
                let body = match unit.target_tokens {
                    Some(target) => prompt_body_sized(plan.spec, model, &unit.text, target),
                    None => prompt_body(plan.spec, model, &unit.text, options.persona_style),
                };
                let estimate = guard.estimate(*price, est_prompt_tokens(&body), options.max_tokens);
                if let Err(reason) = guard.permit(estimate) {
                    halt_reason = Some(format!("{}: {reason}", plan.spec.id));
                    skipped.push(serde_json::json!({
                        "provider": plan.spec.id, "stage": "cost_guard", "reason": reason,
                    }));
                    break 'outer;
                }
                let mut sample =
                    client_call(plan, model, body, &unit.id, rep, *price, options.max_tokens);
                let is_cold = seen.insert((plan.spec.id.to_owned(), model.clone()));
                sample.cold = is_cold;
                sample.route_reason = reason_by_id
                    .get(plan.spec.id)
                    .map(|r| reason_str(r).to_owned());
                let rate_limited = sample.error.as_deref() == Some("RATE_LIMITED");
                guard.record(&sample);
                health.get_mut(plan.spec.id).observe(
                    sample.ok,
                    sample.ttft_ms,
                    rate_limited,
                    sample.cached_tokens,
                    gate.rate_limit_cooldown,
                    Instant::now(),
                );
                samples.push(sample);
                if rate_limited && model_idx + 1 < plan.models.len() {
                    model_idx += 1;
                    notes.push(format!(
                        "{}: rate-limited on {}; fell back to {}",
                        plan.spec.id,
                        plan.models[model_idx - 1].0,
                        plan.models[model_idx].0
                    ));
                    continue;
                }
                rep += 1;
                if guard.spent_usd() >= options.total_cap_usd {
                    halt_reason = Some(format!(
                        "session cost cap ${:.2} reached",
                        options.total_cap_usd
                    ));
                    break 'outer;
                }
            }
        }
    }
    if let Some(reason) = &halt_reason {
        notes.push(format!("run stopped early: {reason}"));
    }

    // --- orcarouter/auto comparison -------------------------------------------
    if options.orca_auto && halt_reason.is_none() && auth_ready(&ORCAROUTER_AUTO_SPEC, &keys) {
        let spec = &ORCAROUTER_AUTO_SPEC;
        let client = BenchClient::new(spec, &keys, timeout);
        let model = ORCAROUTER_AUTO_SPEC.models[0].id;
        'auto: for unit in &units {
            for rep in 0..options.reps {
                let body = match unit.target_tokens {
                    Some(target) => prompt_body_sized(spec, model, &unit.text, target),
                    None => prompt_body(spec, model, &unit.text, options.persona_style),
                };
                // The router chooses the upstream, so the list price is
                // unknown up front. Charge the request cap as the estimate —
                // the worst case the guard can bound — and reconcile from
                // the billed `usage.cost_usd` on each response.
                if let Err(reason) = guard.permit(Some(options.request_cap_usd)) {
                    skipped.push(serde_json::json!({
                        "provider": "orcarouter-auto", "stage": "cost_guard",
                        "reason": reason,
                    }));
                    break 'auto;
                }
                let mut sample =
                    client.chat_stream(model, body, &unit.id, rep, None, options.max_tokens);
                sample.cold = seen.insert((spec.id.to_owned(), model.to_owned()));
                sample.route_reason = Some("auto_router".to_owned());
                let rate_limited = sample.error.as_deref() == Some("RATE_LIMITED");
                guard.record(&sample);
                health.get_mut(spec.id).observe(
                    sample.ok,
                    sample.ttft_ms,
                    rate_limited,
                    sample.cached_tokens,
                    gate.rate_limit_cooldown,
                    Instant::now(),
                );
                samples.push(sample);
                if guard.spent_usd() >= options.total_cap_usd {
                    break 'auto;
                }
            }
        }
    }

    // --- report ---------------------------------------------------------------
    let summaries = summarize_all(&samples);
    let shadow_routes = jev_handle
        .map(|handle| handle.join().unwrap_or_default())
        .unwrap_or_default();
    notes.push("p95 values are indicative only at this sample size".to_owned());
    let report = BenchReport {
        started_at: timestamp_now(),
        prompts: units
            .iter()
            .map(|u| {
                serde_json::json!({
                    "id": u.id,
                    "text": u.text,
                    "target_tokens": u.target_tokens,
                })
            })
            .collect(),
        prompt_style: if options.persona_style {
            "persona".to_owned()
        } else {
            "bare".to_owned()
        },
        max_completion_tokens: options.max_tokens,
        probes,
        samples,
        summaries,
        shadow_routes,
        skipped,
        cost: serde_json::json!({
            "spent_session_usd": guard.spent_usd(),
            "spent_day_usd": guard.day_spent_usd(),
            "spent_month_usd": guard.month_spent_usd(),
            "unpriced_requests": guard.unpriced_requests(),
            "request_cap_usd": options.request_cap_usd,
            "session_cap_usd": options.total_cap_usd,
            "daily_cap_usd": options.daily_cap_usd,
            "monthly_cap_usd": options.monthly_cap_usd,
        }),
        agent_lanes: agent_lanes
            .iter()
            .map(|lane| serde_json::to_value(lane).unwrap_or_default())
            .collect(),
        notes,
    };

    let out = options.out.clone().unwrap_or_else(|| {
        PathBuf::from(format!(
            ".local/provider-bench/report-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ))
    });
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(
        &out,
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    ) {
        Ok(()) => println!("report: {}", out.display()),
        Err(error) => {
            eprintln!("cannot write {}: {error}", out.display());
            return ExitCode::from(1);
        }
    }

    println!(
        "\nprovider         model                                    p50_ttft  p50_total   tok/s    cost"
    );
    for summary in &report.summaries {
        println!(
            "{:<16} {:<40} {:>8} {:>10} {:>8}  ${:.4}",
            summary.provider,
            summary.model,
            summary
                .p50_ttft_ms
                .map(|v| format!("{v}ms"))
                .unwrap_or_else(|| "-".to_owned()),
            summary
                .p50_total_ms
                .map(|v| format!("{v}ms"))
                .unwrap_or_else(|| "-".to_owned()),
            summary
                .mean_decode_tps
                .map(|v| format!("{v:.0}"))
                .unwrap_or_else(|| "-".to_owned()),
            summary.cost_usd_total,
        );
    }
    println!(
        "\nspent ${:.4} session / ${:.4} day / ${:.4} month ({} unpriced); caps ${:.2}/${:?}/${:?}",
        guard.spent_usd(),
        guard.day_spent_usd(),
        guard.month_spent_usd(),
        guard.unpriced_requests(),
        options.total_cap_usd,
        options.daily_cap_usd,
        options.monthly_cap_usd,
    );
    ExitCode::SUCCESS
}

/// One request with the stream path first and a single buffered retry when
/// the provider rejects streaming.
fn client_call(
    plan: &Ready<'_>,
    model: &str,
    body: serde_json::Value,
    prompt_id: &str,
    rep: u32,
    price: Option<(f64, f64)>,
    max_tokens: u32,
) -> kamimusuhi_runtime::provider_bench::BenchSample {
    let mut sample =
        plan.client
            .chat_stream(model, body.clone(), prompt_id, rep, price, max_tokens);
    if !sample.ok
        && matches!(
            sample.error.as_deref(),
            Some("MALFORMED") | Some("HTTP_STATUS_400") | Some("HTTP_STATUS_422")
        )
    {
        let mut retry = plan
            .client
            .chat(model, body, prompt_id, rep, price, max_tokens);
        if retry.ok {
            retry.error = Some("stream_rejected_fell_back".to_owned());
            sample = retry;
        }
    }
    sample
}

fn timestamp_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}
