//! Synthetic, tools-free assessment/one-repair probe. No Runtime/DB is opened.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use kamimusuhi_core::digest::{content_digest, json_digest};
use kamimusuhi_core::persona::{PersonaCore, PersonaTurnInput, ResponseGuidance};
use kamimusuhi_core::routing::LocalityClass;
use kamimusuhi_persona_http::{OpenAiCompatiblePersona, PersonaBackendConfig};
use kamimusuhi_resource_http::tls::TrustAnchors;
use kamimusuhi_runtime::config::{PersonaBackendKind, RuntimeConfig};
use kamimusuhi_runtime::llm_jev::{
    ConversationCoreState, Decision, DecisionEvidence, DecisionProvider, LanguageProviderTelemetry,
    LanguageResponseCandidate, LanguageResponseSelectionRequest, OpenAiDecisionProvider,
    ResponseAssessment, ResponseAssessmentRequest,
};
use serde_json::{Value, json};

const HELP: &str = "shared-experience-probe --config runtime.json --suite cases.json --captures paired.json --identity context.json --out result.json [--limit 4] [--capture-run baseline|candidate]\nUses the selected capture run (default baseline), with up to 8 fixed synthetic cases (default 4), plus two historical-response controls and two post-v1 supplemental duration-evidence calibration inputs. Each selected case and known_bad_warm allows at most one repair; the other controls are assessment only. Logical provider call bound is limit*3+6 (18 by default, 30 at limit 8); the existing transport may make bounded repair requests. No invocation preparation, provider race, fallback, tools, mutation, or canonical DB. Default build uses evidence-scoped repair guidance; build --no-default-features for legacy source compatibility. Output is create-new mode 0600.";
const SELECTED: [&str; 8] = [
    "warm_return",
    "grounded_duration",
    "assistant_only_episode",
    "plain_tired",
    "honest_memory_boundary",
    "resume_open_commitment",
    "corrected_destination_promise",
    "uncertain_user_report",
];
const KNOWN_BAD: &str = "おかえり。仕様の確認、思ったより長引いたからね。少し休もう。";
const DURATION_CALIBRATION: &str = "仕様確認は予定より二時間長引いたね。";

#[derive(Default)]
struct Options {
    config: PathBuf,
    suite: PathBuf,
    captures: PathBuf,
    identity: PathBuf,
    out: PathBuf,
    limit: usize,
    capture_run: String,
}

fn options() -> Result<Option<Options>, &'static str> {
    let mut result = Options {
        limit: 4,
        capture_run: "baseline".into(),
        ..Options::default()
    };
    let mut args = std::env::args().skip(1);
    let mut seen = BTreeSet::new();
    while let Some(flag) = args.next() {
        if flag == "--help" || flag == "-h" {
            println!("{HELP}");
            return Ok(None);
        }
        if !seen.insert(flag.clone()) {
            return Err("duplicate option");
        }
        let value = args.next().ok_or("option requires a value")?;
        match flag.as_str() {
            "--config" => result.config = value.into(),
            "--suite" => result.suite = value.into(),
            "--captures" => result.captures = value.into(),
            "--identity" => result.identity = value.into(),
            "--out" => result.out = value.into(),
            "--limit" => result.limit = value.parse().map_err(|_| "invalid limit")?,
            "--capture-run" => result.capture_run = value,
            _ => return Err("unknown option"),
        }
    }
    if [
        &result.config,
        &result.suite,
        &result.captures,
        &result.identity,
        &result.out,
    ]
    .iter()
    .any(|p| p.as_os_str().is_empty())
        || !(1..=8).contains(&result.limit)
        || !matches!(result.capture_run.as_str(), "baseline" | "candidate")
    {
        return Err("missing path, invalid limit, or invalid capture run");
    }
    if result.out.symlink_metadata().is_ok() {
        return Err("output must not exist");
    }
    Ok(Some(result))
}

fn read_json(path: &Path) -> Result<Value, &'static str> {
    let meta = path.metadata().map_err(|_| "cannot inspect JSON input")?;
    if !meta.is_file() || meta.len() > 8 * 1024 * 1024 {
        return Err("JSON input too large");
    }
    serde_json::from_slice(&std::fs::read(path).map_err(|_| "cannot read JSON input")?)
        .map_err(|_| "invalid JSON input")
}

// Keep the same labels/fields as production dialogue.rs::decision_evidence.
// Deliberately does not add candidate text to the evidence snapshot.
fn decision_evidence(input: &PersonaTurnInput) -> Result<DecisionEvidence, &'static str> {
    let envelope = &input.envelope;
    DecisionEvidence::new(json!({
        "turn": input.context,
        "user_input": input.input,
        "conversation_history": envelope.conversation_history,
        "relationship": envelope.relationship,
        "episodic": envelope.episodic,
        "recalled_evidence": envelope.recalled_evidence,
        "durable_self": envelope.durable_self,
        "continuity_context": envelope.continuity_context,
        "active_policy": envelope.active_policy,
        "observed_runtime": envelope.observed_runtime,
        "mio_observation": envelope.mio_observation,
        "research_findings": envelope.research_findings,
        "body_state": envelope.body_state,
        "library": envelope.library,
        "external_results": envelope.external_results,
        "response_guidance": envelope.response_guidance,
    }))
    .map_err(|_| "invalid synthetic evidence snapshot")
}

fn assessment_request(
    input: &PersonaTurnInput,
    text: &str,
    attempt: u8,
    evidence: &DecisionEvidence,
) -> ResponseAssessmentRequest {
    let state = ConversationCoreState::for_input(
        &ConversationCoreState::default(),
        input.envelope.session.turn_sequence,
        &input.input.text,
    );
    ResponseAssessmentRequest {
        selection: LanguageResponseSelectionRequest {
            user_text: input.input.text.clone(),
            speech_act: state.speech_act.clone(),
            state,
            candidates: vec![LanguageResponseCandidate {
                id: "candidate-0".into(),
                provider: "openai-compatible".into(),
                model: "alias".into(),
                latency_ms: 0,
                response_bytes: text.len(),
                response_digest: content_digest(text.as_bytes()),
                response: text.to_owned(),
                telemetry: LanguageProviderTelemetry::default().snapshot(),
            }],
        },
        evidence: evidence.clone(),
        attempts: BTreeMap::from([("candidate-0".to_owned(), attempt)]),
    }
}

struct Budget {
    used: usize,
    max: usize,
}
impl Budget {
    fn take(&mut self) -> Result<(), &'static str> {
        if self.used >= self.max {
            return Err("logical call budget exhausted");
        }
        self.used += 1;
        Ok(())
    }
}

fn assess(
    decision: &OpenAiDecisionProvider,
    input: &PersonaTurnInput,
    text: &str,
    attempt: u8,
    evidence: &DecisionEvidence,
    budget: &mut Budget,
) -> Result<ResponseAssessment, String> {
    budget.take().map_err(str::to_owned)?;
    decision
        .assess_responses(&assessment_request(input, text, attempt, evidence))
        .map_err(|e| format!("assessment_{}", e.code()))
}

fn one_probe(
    id: &str,
    input: &PersonaTurnInput,
    raw: &str,
    allow_repair: bool,
    persona: &dyn PersonaCore,
    decision: &OpenAiDecisionProvider,
    budget: &mut Budget,
) -> Value {
    let started = Instant::now();
    let context_digest = json_digest(&json!(input));
    let mut record = json!({
        "probe_id":id,"input_digest":content_digest(input.input.text.as_bytes()),
        "context_digest":context_digest,"raw_response":raw,"raw_response_digest":content_digest(raw.as_bytes()),
        "repair_allowed":allow_repair,"repair_count":0,"delivery_eligible":false,
        "assessments":[],"final_response":raw,"final_response_digest":content_digest(raw.as_bytes()),
        "status":"unverified","error_code":null,
    });
    let work = (|| -> Result<(), String> {
        if raw.trim().is_empty() || raw.len() > 16 * 1024 {
            return Err("invalid_raw_response".into());
        }
        let evidence = decision_evidence(input).map_err(str::to_owned)?;
        record["evidence_digest"] = json!(evidence.snapshot_digest);
        let first = assess(decision, input, raw, 0, &evidence, budget)?;
        record["assessments"]
            .as_array_mut()
            .unwrap()
            .push(json!(first));
        let mut final_gate = first.gate.decision;
        if final_gate == Decision::Retry && allow_repair {
            let mut repair_input = input.clone();
            #[cfg(feature = "scope-guidance")]
            let repair_instruction = first.repair_reason.instruction_for_evidence(&evidence);
            #[cfg(not(feature = "scope-guidance"))]
            let repair_instruction = first.repair_reason.as_instruction().map(str::to_owned);
            let repair = repair_instruction.as_deref().unwrap_or(
                "先の候補を見直し、相手の質問と提示された根拠に合う短い日本語の回答を作り直してください。"
            );
            // Match production: grounding repair rebuilds from evidence without
            // presenting the unsupported candidate prose again; retain its digest.
            #[cfg(feature = "scope-guidance")]
            let previous_response = (first.repair_reason
                != kamimusuhi_runtime::llm_jev::RepairReason::Grounding)
                .then(|| raw.to_owned());
            #[cfg(not(feature = "scope-guidance"))]
            let previous_response = Some(raw.to_owned());
            let instruction = input.envelope.response_guidance.as_ref().map_or_else(
                || repair.to_owned(),
                |g| format!("{}\n{repair}", g.instruction),
            );
            repair_input.envelope.response_guidance = Some(ResponseGuidance {
                reason_code: first.repair_reason.as_str().to_owned(),
                instruction,
                previous_response_digest: Some(content_digest(raw.as_bytes())),
                previous_response,
            });
            record["repair_input_digest"] = json!(json_digest(&json!(repair_input)));
            budget.take().map_err(str::to_owned)?;
            let generated = persona
                .turn(repair_input)
                .map_err(|_| "repair_generation_failed".to_owned())?;
            record["repair_count"] = json!(1);
            record["discarded_proposals"] = json!(generated.proposals.len());
            if generated.response_intent.trim().is_empty()
                || generated.response_intent.len() > 16 * 1024
                || !generated.tool_calls.is_empty()
            {
                return Err("invalid_repair_response_or_tool".into());
            }
            record["final_response"] = json!(generated.response_intent);
            record["final_response_digest"] =
                json!(content_digest(generated.response_intent.as_bytes()));
            let second = assess(
                decision,
                input,
                &generated.response_intent,
                1,
                &evidence,
                budget,
            )?;
            final_gate = second.gate.decision;
            record["assessments"]
                .as_array_mut()
                .unwrap()
                .push(json!(second));
        }
        record["delivery_eligible"] = json!(final_gate == Decision::Accept);
        record["status"] = json!(if final_gate == Decision::Accept {
            "accepted_by_model_gate"
        } else {
            "not_accepted_by_model_gate"
        });
        Ok(())
    })();
    if let Err(error) = work {
        record["error_code"] = json!(error);
    }
    record["elapsed_ms"] = json!(started.elapsed().as_millis());
    record
}

fn create_output(path: &Path) -> Result<File, &'static str> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| "cannot create protected output")
}

fn run() -> Result<(), &'static str> {
    let Some(o) = options()? else {
        return Ok(());
    };
    let suite = read_json(&o.suite)?;
    let captures = read_json(&o.captures)?;
    let identity = read_json(&o.identity)?;
    if !identity.is_object() || identity.to_string().len() > 64 * 1024 {
        return Err("bounded identity context object required");
    }
    let mut config =
        RuntimeConfig::load(&o.config).map_err(|_| "cannot load runtime configuration")?;
    if config.persona.backend != PersonaBackendKind::OpenaiCompatible {
        return Err("HTTP persona backend required");
    }
    let p = config
        .persona
        .provider
        .as_mut()
        .ok_or("HTTP persona provider required")?;
    if p.locality == LocalityClass::InProcess {
        return Err("HTTP decision backend cannot declare in-process locality");
    }
    p.timeout_ms = p.timeout_ms.min(60_000);
    let backend = PersonaBackendConfig::new(p.backend_id, p.base_url.clone(), p.model.clone())
        .with_auth_env(p.auth_env.clone())
        .with_timeout_ms(p.timeout_ms)
        .with_extra_body(p.extra_body.clone())
        .with_trust_anchors(match &p.tls_root_ca_path {
            Some(path) => TrustAnchors::PemFile(path.clone()),
            None => TrustAnchors::Webpki,
        });
    backend
        .validate()
        .map_err(|_| "invalid decision backend configuration")?;
    let decision = OpenAiDecisionProvider::new(OpenAiCompatiblePersona::new(backend), p.locality);
    let requested_model = p.model.clone();
    let sampling_digest = json_digest(&json!({"reasoning":p.reasoning,"extra_body":p.extra_body}));
    let persona = config
        .build_replay_persona()
        .map_err(|_| "cannot build tools-free persona")?;
    decision.set_identity_context(identity.clone());
    persona.set_identity_context(identity);
    let seed = config
        .persona_seed()
        .map_err(|_| "cannot load persona seed")?;
    let cases = suite["cases"].as_array().ok_or("suite cases missing")?;
    let outputs = captures[&o.capture_run]["outputs"]
        .as_array()
        .ok_or("selected capture run outputs missing")?;
    let mut inputs = BTreeMap::new();
    for id in SELECTED {
        let c = cases
            .iter()
            .find(|c| c["case_id"] == id)
            .ok_or("required synthetic case missing")?;
        let mut input: PersonaTurnInput = serde_json::from_value(c["turn_input"].clone())
            .map_err(|_| "invalid synthetic turn")?;
        if input.envelope.persona_seed.is_none() {
            input.envelope.persona_seed = seed.clone();
        }
        inputs.insert(id, input);
    }
    let mut selected = Vec::new();
    for id in SELECTED.into_iter().take(o.limit) {
        let output = outputs
            .iter()
            .find(|r| r["case_id"] == id)
            .ok_or("required capture missing")?;
        let text = output["text"].as_str().ok_or("capture text missing")?;
        let input = &inputs[id];
        if output["context_digest"] != json_digest(&json!(input)) {
            return Err("capture context digest mismatch");
        }
        selected.push((id.to_owned(), input.clone(), text.to_owned(), true));
    }
    selected.push((
        "known_bad_warm".into(),
        inputs["warm_return"].clone(),
        KNOWN_BAD.into(),
        true,
    ));
    selected.push((
        "known_text_grounded_duration_control".into(),
        inputs["grounded_duration"].clone(),
        KNOWN_BAD.into(),
        false,
    ));
    // Post-v1 supplemental diagnostics: identical candidate, different evidence.
    // These do not alter the fixed suite/rubric or the original six probe inputs.
    selected.push((
        "duration_claim_unverified_context".into(),
        inputs["warm_return"].clone(),
        DURATION_CALIBRATION.into(),
        false,
    ));
    selected.push((
        "duration_claim_supported_context".into(),
        inputs["grounded_duration"].clone(),
        DURATION_CALIBRATION.into(),
        false,
    ));
    let mut file = create_output(&o.out)?;
    let mut budget = Budget {
        used: 0,
        max: o.limit * 3 + 6,
    };
    let mut records = Vec::new();
    for (id, input, text, allow_repair) in selected {
        records.push(one_probe(
            &id,
            &input,
            &text,
            allow_repair,
            persona.as_ref(),
            &decision,
            &mut budget,
        ));
        println!("probe completed: {id}");
    }
    let result = json!({
        "format_version":1,"probe_kind":"single_candidate_assessment_and_one_repair",
        "scope":"uses production typed decision criteria/composition and tools-free Persona repair; excludes invocation preparation, provider race, fallback, emission, and all DB/state mutation",
        "requested_model":requested_model,"actual_returned_model":null,"usage":null,
        "sampling_digest":sampling_digest,"suite_digest":json_digest(&suite),"captures_digest":json_digest(&captures),
        "decision_source_digest":content_digest(include_bytes!("../../../../crates/kamimusuhi-runtime/src/llm_jev/assessment.rs")),
        "persona_source_digest":content_digest(include_bytes!("../../../../crates/kamimusuhi-persona-http/src/lib.rs")),
        "capture_run":o.capture_run,"selected_case_limit":o.limit,
        "scope_guidance_enabled":cfg!(feature = "scope-guidance"),
        "supplemental_calibration":{
            "introduced_after":"v1 probe execution",
            "role":"diagnostic only; fixed eight-case rubric and main comparison unchanged",
            "probe_ids":["duration_claim_unverified_context","duration_claim_supported_context"],
            "candidate_text_identical":true,"repair_allowed":false,
            "interpretation":"compare factual grounding separately from task_fit under the different current user inputs"
        },
        "logical_provider_calls":budget.used,"max_logical_provider_calls":budget.max,
        "transport_timeout_ms_cap":60000,"tools_offered":false,"canonical_db_opened":false,
        "limits":["model gate is a model judgment, not human rating or proof of truth","logical call bound excludes existing adapter's bounded transport/JSON-repair calls","known_text_grounded_duration_control has a different current user request; evaluate its task_fit separately","capture baseline/candidate naming means first/second capture repeat, not implementation version"],
        "results":records,
    });
    serde_json::to_writer_pretty(&mut file, &result).map_err(|_| "cannot serialize output")?;
    file.write_all(b"\n")
        .and_then(|_| file.sync_all())
        .map_err(|_| "cannot persist output")?;
    println!(
        "completed {} synthetic inputs; {} logical calls",
        result["results"].as_array().unwrap().len(),
        budget.used
    );
    Ok(())
}

fn main() {
    if let Err(code) = run() {
        eprintln!("shared-experience-probe: {code}");
        std::process::exit(2);
    }
}
