//! Tools-free, DB-free paired captures of explicitly supplied synthetic turns.
use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use kamimusuhi_core::digest::{content_digest, json_digest};
use kamimusuhi_core::persona::PersonaTurnInput;
use kamimusuhi_runtime::config::{PersonaBackendKind, RuntimeConfig};
use kamimusuhi_runtime::continuity_eval::{
    CaptureKind, ComparisonInput, ConditionComponent, ContinuityCase, HumanAxis, RecordedOutput,
    RecordedRun, RunConditions, compare,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Suite {
    format_version: u32,
    suite_id: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    case_id: String,
    turn_input: PersonaTurnInput,
    #[serde(default)]
    required_text: Vec<String>,
    #[serde(default)]
    forbidden_text: Vec<String>,
    #[serde(default)]
    required_evidence_ids: Vec<String>,
    #[serde(default)]
    commitment_text: Vec<String>,
    #[serde(default)]
    human_axes: BTreeSet<HumanAxis>,
}

#[derive(Default)]
struct Options {
    config: PathBuf,
    cases: PathBuf,
    out: PathBuf,
    meta: PathBuf,
    identity_context: Option<PathBuf>,
    baseline_model: Option<String>,
    candidate_model: Option<String>,
    limit: usize,
    max_requests: usize,
}

#[derive(Serialize)]
struct CaptureMeta {
    run_id: String,
    case_id: String,
    latency_ms: u128,
    requested_model: String,
    actual_model: Option<String>,
    usage: Option<Value>,
    discarded_proposals: usize,
}

fn usage() -> &'static str {
    "Usage: continuity-capture --config runtime.json --cases synthetic.json --out paired.json --meta capture-meta.json [--identity-context context.json] [--limit 8] [--max-requests 16] [--baseline-model NAME] [--candidate-model NAME]\nNo individual DB is opened. Model output is retained; protect output files. Tools are disabled."
}

fn options() -> Result<Option<Options>, String> {
    let mut o = Options {
        limit: 8,
        max_requests: 16,
        ..Options::default()
    };
    let mut seen = BTreeSet::new();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--help" || flag == "-h" {
            println!("{}", usage());
            return Ok(None);
        }
        if !seen.insert(flag.clone()) {
            return Err("repeated option".into());
        }
        let value = args.next().ok_or("option needs a value")?;
        match flag.as_str() {
            "--config" => o.config = value.into(),
            "--cases" => o.cases = value.into(),
            "--out" => o.out = value.into(),
            "--meta" => o.meta = value.into(),
            "--identity-context" => o.identity_context = Some(value.into()),
            "--baseline-model" => o.baseline_model = Some(value),
            "--candidate-model" => o.candidate_model = Some(value),
            "--limit" => o.limit = value.parse().map_err(|_| "invalid limit")?,
            "--max-requests" => {
                o.max_requests = value.parse().map_err(|_| "invalid request bound")?
            }
            _ => return Err("unknown option".into()),
        }
    }
    if [&o.config, &o.cases, &o.out, &o.meta]
        .iter()
        .any(|p| p.as_os_str().is_empty())
        || !(1..=64).contains(&o.limit)
        || !(2..=128).contains(&o.max_requests)
        || o.out == o.meta
        || [&o.baseline_model, &o.candidate_model].iter().any(|m| {
            m.as_ref()
                .is_some_and(|m| m.trim().is_empty() || m.len() > 256)
        })
    {
        return Err("missing path or invalid capture bound/model".into());
    }
    // Outputs are append-once captures, never an overwrite of an input/config.
    if o.out.symlink_metadata().is_ok() || o.meta.symlink_metadata().is_ok() {
        return Err("capture outputs must not already exist".into());
    }
    Ok(Some(o))
}

fn read_json(path: &Path, max: u64) -> Result<Value, String> {
    let m = std::fs::metadata(path).map_err(|_| "cannot inspect JSON input")?;
    if !m.is_file() || m.len() > max {
        return Err("JSON input exceeds bound".into());
    }
    serde_json::from_slice(&std::fs::read(path).map_err(|_| "cannot read JSON input")?)
        .map_err(|_| "invalid JSON input".into())
}

fn validate_turn_keys(raw: &Value) -> Result<(), String> {
    fn keys(value: &Value, allowed: &[&str]) -> bool {
        value
            .as_object()
            .is_some_and(|m| m.keys().all(|k| allowed.contains(&k.as_str())))
    }
    for case in raw["cases"].as_array().ok_or("cases must be an array")? {
        let input = &case["turn_input"];
        if !keys(input, &["context", "input", "envelope"])
            || !keys(
                &input["context"],
                &["individual_id", "session_id", "turn_id"],
            )
            || !keys(&input["input"], &["evidence_id", "text"])
        {
            return Err("unknown turn/context/input field".into());
        }
        if let Some(envelope) = input.get("envelope")
            && !keys(
                envelope,
                &[
                    "continuity",
                    "durable_self",
                    "continuity_context",
                    "relationship",
                    "episodic",
                    "recalled_evidence",
                    "active_policy",
                    "self_notes",
                    "library",
                    "external_results",
                    "organ_signals",
                    "conversation_history",
                    "conversation_core",
                    "response_guidance",
                    "observed_runtime",
                    "mio_observation",
                    "research_findings",
                    "reference_material",
                    "body_state",
                    "persona_seed",
                    "session",
                ],
            )
        {
            return Err("unknown envelope field".into());
        }
    }
    Ok(())
}

fn cases(suite: &Suite) -> Vec<ContinuityCase> {
    suite
        .cases
        .iter()
        .map(|c| ContinuityCase {
            case_id: c.case_id.clone(),
            input: c.turn_input.input.text.clone(),
            context_digest: json_digest(&json!(c.turn_input)),
            required_text: c.required_text.clone(),
            forbidden_text: c.forbidden_text.clone(),
            required_evidence_ids: c.required_evidence_ids.clone(),
            commitment_text: c.commitment_text.clone(),
            human_axes: c.human_axes.clone(),
        })
        .collect()
}

fn conditions(config: &RuntimeConfig, suite: &Suite) -> Result<RunConditions, String> {
    let p = config
        .persona
        .provider
        .as_ref()
        .ok_or("HTTP provider is required")?;
    Ok(RunConditions {
        model: p.model.clone(),
        prompt_digest: json_digest(
            &json!({"compiled_prompt":content_digest(include_bytes!("../../../kamimusuhi-persona-http/src/lib.rs")),"system_instruction":p.system_instruction,"avatar_name":config.avatar_name,"avatar_aliases":config.avatar_aliases,"avatar_self_description":config.avatar_self_description}),
        ),
        sampling_digest: json_digest(&json!({"reasoning":p.reasoning,"extra_body":p.extra_body})),
        tools_digest: json_digest(&json!({"tools":[],"execution":"disabled"})),
        memory_digest: json_digest(&json!(
            suite
                .cases
                .iter()
                .map(|c| &c.turn_input)
                .collect::<Vec<_>>()
        )),
    })
}

fn run_capture(
    config: &RuntimeConfig,
    suite: &Suite,
    run_id: &str,
    identity: &Value,
    meta: &mut Vec<CaptureMeta>,
) -> Result<RecordedRun, String> {
    let conditions = conditions(config, suite)?;
    let persona = config
        .build_replay_persona()
        .map_err(|_| "cannot construct tools-free provider")?;
    if !identity.is_null() {
        persona.set_identity_context(identity.clone());
    }
    let mut outputs = Vec::new();
    for c in &suite.cases {
        let start = Instant::now();
        let result = persona.turn(c.turn_input.clone()).map_err(|_| {
            format!(
                "provider capture failed for run {run_id} case {}",
                c.case_id
            )
        })?;
        if result.response_intent.trim().is_empty()
            || !result.tool_calls.is_empty()
            || result.response_intent.len() > 128 * 1024
        {
            return Err("empty/oversized response or unexpected tool execution".into());
        }
        let cited_evidence_ids = c
            .required_evidence_ids
            .iter()
            .filter(|id| result.response_intent.contains(id.as_str()))
            .cloned()
            .collect();
        outputs.push(RecordedOutput {
            case_id: c.case_id.clone(),
            input: c.turn_input.input.text.clone(),
            context_digest: json_digest(&json!(c.turn_input)),
            text: result.response_intent,
            cited_evidence_ids,
            human_reviews: Vec::new(),
        });
        meta.push(CaptureMeta {
            run_id: run_id.into(),
            case_id: c.case_id.clone(),
            latency_ms: start.elapsed().as_millis(),
            requested_model: conditions.model.clone(),
            actual_model: None,
            usage: None,
            discarded_proposals: result.proposals.len(),
        });
    }
    Ok(RecordedRun {
        run_id: run_id.into(),
        capture_kind: CaptureKind::ModelOutput,
        condition_digest: conditions.digest(),
        conditions,
        outputs,
    })
}

fn write_new(path: &Path, value: &Value) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "cannot create capture output")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "cannot protect capture output")?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "cannot serialize capture")?;
    f.write_all(&bytes)
        .and_then(|_| f.write_all(b"\n"))
        .and_then(|_| f.sync_all())
        .map_err(|_| "cannot persist capture".into())
}

fn run() -> Result<(), String> {
    let Some(o) = options()? else { return Ok(()) };
    let raw = read_json(&o.cases, 4 * 1024 * 1024)?;
    validate_turn_keys(&raw)?;
    let mut suite: Suite =
        serde_json::from_value(raw).map_err(|_| "invalid synthetic suite schema")?;
    if suite.format_version != 1 || suite.cases.is_empty() || suite.cases.len() > 64 {
        return Err("invalid suite version/size".into());
    }
    suite.cases.truncate(o.limit);
    if suite.cases.len() * 2 > o.max_requests {
        return Err("two-run capture exceeds max-requests".into());
    }
    for c in &suite.cases {
        if c.turn_input.context.individual_id.is_nil()
            || c.turn_input.context.session_id.is_nil()
            || c.turn_input.context.turn_id.is_nil()
            || c.turn_input.input.evidence_id.is_nil()
            || serde_json::to_vec(&c.turn_input)
                .map_err(|_| "invalid turn")?
                .len()
                > 512 * 1024
        {
            return Err("invalid turn identity or size".into());
        }
    }
    let mut baseline_config =
        RuntimeConfig::load(&o.config).map_err(|_| "cannot load runtime configuration")?;
    if baseline_config.persona.backend != PersonaBackendKind::OpenaiCompatible {
        return Err("model-output capture requires an HTTP Persona".into());
    }
    if let Some(seed) = baseline_config
        .persona_seed()
        .map_err(|_| "cannot load configured persona seed")?
    {
        for case in &mut suite.cases {
            if case.turn_input.envelope.persona_seed.is_none() {
                case.turn_input.envelope.persona_seed = Some(seed.clone());
            }
        }
    }
    // Timeout bounds each transport attempt; the adapter may perform bounded
    // repair attempts. max-requests bounds logical Persona invocations.
    let provider = baseline_config
        .persona
        .provider
        .as_mut()
        .ok_or("HTTP provider is required")?;
    provider.timeout_ms = provider.timeout_ms.min(120_000);
    if let Some(model) = o.baseline_model {
        provider.model = model;
    }
    let mut candidate_config = baseline_config.clone();
    if let Some(model) = o.candidate_model {
        candidate_config.persona.provider.as_mut().unwrap().model = model;
    }
    let identity = match o.identity_context {
        Some(path) => read_json(&path, 1024 * 1024)?,
        None => Value::Null,
    };
    let make_placeholder = |config: &RuntimeConfig, id: &str| -> Result<RecordedRun, String> {
        let conditions = conditions(config, &suite)?;
        Ok(RecordedRun {
            run_id: id.into(),
            capture_kind: CaptureKind::ModelOutput,
            condition_digest: conditions.digest(),
            conditions,
            outputs: cases(&suite)
                .iter()
                .map(|c| RecordedOutput {
                    case_id: c.case_id.clone(),
                    input: c.input.clone(),
                    context_digest: c.context_digest.clone(),
                    text: "preflight-only".into(),
                    cited_evidence_ids: vec![],
                    human_reviews: vec![],
                })
                .collect(),
        })
    };
    let allowed = if baseline_config.persona.provider.as_ref().unwrap().model
        != candidate_config.persona.provider.as_ref().unwrap().model
    {
        BTreeSet::from([ConditionComponent::Model])
    } else {
        BTreeSet::new()
    };
    let mut paired = ComparisonInput {
        format_version: 1,
        suite_id: suite.suite_id.clone(),
        cases: cases(&suite),
        allowed_condition_changes: allowed,
        baseline: make_placeholder(&baseline_config, "baseline")?,
        candidate: make_placeholder(&candidate_config, "candidate")?,
    };
    compare(&paired).map_err(|_| "invalid evaluation contract before provider calls")?;
    let mut captures = Vec::new();
    paired.baseline = run_capture(
        &baseline_config,
        &suite,
        "baseline",
        &identity,
        &mut captures,
    )?;
    paired.candidate = run_capture(
        &candidate_config,
        &suite,
        "candidate",
        &identity,
        &mut captures,
    )?;
    let report = compare(&paired).map_err(|_| "captured output failed comparison schema")?;
    write_new(&o.out, &json!(paired))?;
    write_new(
        &o.meta,
        &json!({"format_version":1,"capture_kind":"model_output","suite_id":suite.suite_id,"logical_requests":captures.len(),"max_logical_requests":o.max_requests,"transport_timeout_ms_cap":120000,"identity_context_supplied":!identity.is_null(),"captures":captures,"comparison_status":report.comparison_status,"limits":["same synthetic context repeated; not old/new build causal comparison","actual returned model and usage unavailable through PersonaCore; null means unknown","no human reviews collected; AI judgments must remain separate","max-requests bounds logical Persona invocations; existing adapter can make bounded repair HTTP calls","no tools offered, no canonical DB opened, proposals discarded"]}),
    )?;
    println!(
        "{}",
        json!({"captured_cases":paired.cases.len(),"logical_requests":paired.cases.len()*2,"comparison_status":report.comparison_status})
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
