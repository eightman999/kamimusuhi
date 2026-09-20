//! Kamimusuhi runtime binary.
//!
//! ```text
//! kamimusuhi-runtime init            --dir <path> [--resource <impl>] [--seed <n>]
//! kamimusuhi-runtime inspect         --dir <path> [--seed <n>]
//! kamimusuhi-runtime demo-continuity --dir <path> --phase first|resume
//!                                    [--resource <impl>] [--seed <n>]
//! kamimusuhi-runtime chat           --dir <path> [--subject <id>]
//! kamimusuhi-runtime talk           --dir <path> --message <text>
//! ```
//!
//! Reports go to stdout as JSON so a harness can read them; a phase run by one
//! process reports to the outside world this way rather than by handing
//! anything to the next process.
//!
//! `--id-seed` fixes the ID sequence; `--clock` chooses real or pinned time.
//! They are separate because a reproducible fixture usually wants stable IDs
//! *and* real durations once network calls are involved. `--seed` remains as
//! shorthand for both being deterministic.
//!
//! Two processes sharing a runtime directory must use different ID seeds. The
//! same seed replays the same IDs, which the runtime detects and refuses.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::routing::{LocalityClass, PrivacyConstraint, Urgency};
use kamimusuhi_runtime::config::GENERAL_SLOT;
use kamimusuhi_runtime::dialogue::{DialogueSession, MAX_INPUT_BYTES};
use kamimusuhi_runtime::llm_jev::{
    GROKBOT_API_KEY_ENV, LLM_BASE_URL_ENV, LLM_MODEL_ENV, LLM_PROVIDER_ENV,
};
use kamimusuhi_runtime::mio::MioBinding;
use kamimusuhi_runtime::runtime::ClockMode;
use kamimusuhi_runtime::scenario::ScenarioOptions;
use kamimusuhi_runtime::{
    DemoPhase, PersonaBackendKind, PersonaProviderConfig, PersonaSetting, ResourceImplementation,
    Runtime, RuntimeError, RuntimeOptions, inspect, scenario,
};

/// A stable backend ID for an endpoint the operator named on the command line.
///
/// Derived from the endpoint and model rather than minted, so restarting
/// against the same endpoint keeps the same attribution in the trace.
fn persona_backend_id_for(base_url: &str, model: &str) -> PersonaBackendId {
    let digest = content_digest(format!("{base_url}|{model}").as_bytes());
    let bytes: Vec<u8> = digest
        .trim_start_matches("sha256:")
        .as_bytes()
        .chunks(2)
        .take(16)
        .filter_map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect();
    let mut raw = [0_u8; 16];
    raw[..bytes.len().min(16)].copy_from_slice(&bytes[..bytes.len().min(16)]);
    let value = u128::from_be_bytes(raw);
    PersonaBackendId::from_u128(if value == 0 { 1 } else { value })
}

/// Resolve the language-organ environment contract into the existing Persona
/// provider namespace. The key value is never read here; only the credential
/// variable name is placed in runtime configuration.
fn persona_setting_from_env(
    config: &kamimusuhi_runtime::RuntimeConfig,
) -> Result<Option<PersonaSetting>, RuntimeError> {
    let provider = match std::env::var(LLM_PROVIDER_ENV) {
        Ok(value) if !value.trim().is_empty() => value.to_ascii_lowercase(),
        _ => return Ok(None),
    };
    if provider == "mock" {
        return Ok(None);
    }
    if !matches!(provider.as_str(), "grokbot" | "hai" | "openai-compatible") {
        return Err(RuntimeError::Usage(format!(
            "unsupported {LLM_PROVIDER_ENV} value {provider:?}"
        )));
    }
    let base_url = std::env::var(LLM_BASE_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuntimeError::Usage(format!("{LLM_BASE_URL_ENV} is required")))?;
    let model = std::env::var(LLM_MODEL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuntimeError::Usage(format!("{LLM_MODEL_ENV} is required")))?;
    let existing = config.persona.provider.as_ref();
    let auth_env = if provider == "grokbot" {
        Some(GROKBOT_API_KEY_ENV.to_owned())
    } else {
        existing.and_then(|provider| provider.auth_env.clone())
    };
    Ok(Some(PersonaSetting {
        backend: PersonaBackendKind::OpenaiCompatible,
        provider: Some(PersonaProviderConfig {
            backend_id: persona_backend_id_for(&base_url, &model),
            locality: LocalityClass::External,
            base_url,
            model,
            auth_env,
            timeout_ms: existing.map_or(60_000, |provider| provider.timeout_ms),
            tls_root_ca_path: existing.and_then(|provider| provider.tls_root_ca_path.clone()),
            system_instruction: existing.and_then(|provider| provider.system_instruction.clone()),
        }),
        seed: config.persona.seed.clone(),
    }))
}

const USAGE: &str = "\
kamimusuhi-runtime <command> [options]

commands:
  init              prepare a runtime directory; creates an individual only
                    when the canonical database holds none
  inspect           read-only report of identity, lineage, memory, Library
                    and resource calls
  demo-continuity   run one phase of the deterministic restart scenario
  talk              answer --message once; JSON on stdout
  chat              interactive text conversation; /quit or EOF to stop,
                    /status /memory /reflect /rollback /context /help
  reflect           run one reflection cycle now; JSON report on stdout
  rollback          move the derived lane back one activation; JSON on stdout
  activate          gate + apply a pending C0 proposal (--proposal-id <hex>)
  research-check    verify bundled research source files; no runtime writes

options:
  --dir <path>      runtime directory (required)
  --source-root <path>  repository root for research-check (default: .)
  --message <text>  talk only: the current user input
  --subject <id>    talk/chat interlocutor key (default: local-user)
                    Selects a separate history; not authentication.
  --mio-url <url>   MIO coordinator base URL (read-only observation bridge)
  --mio-experiment <id>  expected MIO experiment; required with --mio-url
  --mio-genome <id> pinned MIO genome; required with --mio-url
  --mio-max-age-secs <n>  freshness threshold (1-86400, default 300)
  --no-mio         disconnect the configured MIO observation bridge
  --phase <p>       demo-continuity only: first | resume
  --resource <i>    implementation filling the general slot: fake-a |
                    fake-b | fake-unavailable | openai-compatible
  --id-seed <n>     deterministic ID sequence
  --clock <c>       system | fixed
  --seed <n>        shorthand for --id-seed <n> --clock fixed
  --privacy <p>     how far this turn's material may travel:
                    local-only | no-external-service | unconstrained
  --urgency <u>     whether anything is waiting on this turn:
                    interactive (default) | background. A resource declared
                    `slow` is only ever reachable from a background turn.
  --persona <p>     Persona Core backend: fake | openai-compatible.
                    A model-backed persona needs a `persona.provider` entry in
                    runtime.json; the Persona namespace is separate from
                    `resources` and is never offered to the router.
  --persona-url <u> base URL of the persona endpoint, e.g.
                    http://127.0.0.1:11434/v1
  --persona-model <m>  model name to ask the persona endpoint for
  --persona-auth-env <name>  environment variable holding the API key (never its value)
  --persona-locality <l>  local-host | local-network | external (default: external)
  --debug-context   talk/chat: emit the assembled workspace context
                    (stderr in chat; a `debug_context` field in talk's JSON)
  --debug           talk/chat: emit secret-free K-CORE, Jev and LLM trace
  --proposal-id <hex>  activate only: the pending C0 proposal to gate+apply
";

fn main() -> ExitCode {
    match run() {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("kamimusuhi-runtime: {error}");
            // 2 is reserved for usage so a harness can tell "you asked wrong"
            // from "the runtime refused".
            match error {
                RuntimeError::Usage(_) => ExitCode::from(2),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn run() -> Result<String, RuntimeError> {
    let mut args = std::env::args().skip(1);
    let command = args
        .next()
        .ok_or_else(|| RuntimeError::Usage(format!("no command given\n\n{USAGE}")))?;
    let options = Options::parse(args)?;
    if options.source_root.is_some() && command != "research-check" {
        return Err(RuntimeError::Usage(
            "--source-root is only supported by research-check".to_owned(),
        ));
    }
    if !matches!(command.as_str(), "talk" | "chat" | "reflect" | "activate")
        && (options.message.is_some() || options.subject.is_some() || options.mio_options_present())
    {
        return Err(RuntimeError::Usage(
            "--message, --subject and MIO flags are dialogue options".to_owned(),
        ));
    }

    match command.as_str() {
        "research-check" => {
            let catalog = kamimusuhi_runtime::research::ResearchCatalog::bundled()?;
            catalog.validate_sources(std::path::Path::new(
                options.source_root.as_deref().unwrap_or("."),
            ))?;
            encode(&serde_json::json!({"status": "sources_verified"}))
        }
        "init" => {
            let runtime = Runtime::init(
                options.dir()?,
                options.runtime_options(),
                options.resource.unwrap_or(ResourceImplementation::FakeA),
            )?;
            let report = inspect::inspect(&runtime)?;
            runtime.stopping();
            encode(&report)
        }
        "inspect" => {
            // Opened without claiming a writer epoch: inspection is a read.
            let runtime = Runtime::open(options.dir()?, options.runtime_options())?;
            let report = inspect::inspect(&runtime)?;
            runtime.stopping();
            encode(&report)
        }
        "demo-continuity" => {
            let phase = options.phase.ok_or_else(|| {
                RuntimeError::Usage("demo-continuity requires --phase first|resume".to_owned())
            })?;
            let mut runtime = Runtime::open(options.dir()?, options.runtime_options())?;
            // Replacing the resource or the persona is a config rewrite and
            // nothing else: no canonical write, no new individual, no lineage
            // event. The two namespaces are rewritten independently.
            let mut config = runtime.config().clone();
            let mut changed = false;
            if let Some(resource) = options.resource {
                config.set_implementation(GENERAL_SLOT, resource);
                changed = true;
            }
            if let Some(persona) = options.persona {
                config.persona = options.persona_setting(persona, &config)?;
                changed = true;
            }
            if changed {
                runtime.save_config(config)?;
            }
            let report = scenario::run(
                &mut runtime,
                phase,
                ScenarioOptions {
                    privacy: options.privacy.unwrap_or_default(),
                    urgency: options.urgency.unwrap_or_default(),
                },
            )?;
            runtime.stopping();
            encode(&report)
        }
        "talk" | "chat" => run_dialogue(&command, &options),
        "reflect" | "rollback" | "activate" => run_c0_command(&command, &options),
        "--help" | "-h" | "help" => Ok(USAGE.to_owned()),
        other => Err(RuntimeError::Usage(format!(
            "unknown command {other:?}\n\n{USAGE}"
        ))),
    }
}

fn run_dialogue(command: &str, options: &Options) -> Result<String, RuntimeError> {
    if command == "talk" && options.message.is_none() {
        return Err(RuntimeError::Usage("talk requires --message".to_owned()));
    }
    if command == "chat" && options.message.is_some() {
        return Err(RuntimeError::Usage(
            "--message is only supported by talk".to_owned(),
        ));
    }
    if let Some(message) = &options.message
        && (message.trim().is_empty() || message.len() > MAX_INPUT_BYTES)
    {
        return Err(RuntimeError::Usage(format!(
            "input must be nonempty and at most {MAX_INPUT_BYTES} UTF-8 bytes"
        )));
    }
    if options.phase.is_some() || options.resource.is_some() || options.urgency.is_some() {
        return Err(RuntimeError::Usage(
            "talk/chat use the Persona Core; --phase, --resource and --urgency are not supported"
                .to_owned(),
        ));
    }
    if options.persona.is_none()
        && (options.persona_url.is_some()
            || options.persona_model.is_some()
            || options.persona_auth_env.is_some()
            || options.persona_locality.is_some())
    {
        return Err(RuntimeError::Usage(
            "persona endpoint flags require --persona openai-compatible".to_owned(),
        ));
    }
    let mut runtime = Runtime::open(options.dir()?, options.runtime_options())?;
    let mut config = runtime.config().clone();
    let mut env_persona_changed = false;
    if let Some(backend) = options.persona {
        config.persona = options.persona_setting(backend, &config)?;
    } else if let Some(setting) = persona_setting_from_env(&config)? {
        config.persona = setting;
        env_persona_changed = true;
    }
    if options.mio_options_present() {
        config.mio = options.mio_setting(config.mio.as_ref())?;
    }
    if config.persona.backend == PersonaBackendKind::Fake
        && options.persona != Some(PersonaBackendKind::Fake)
    {
        return Err(RuntimeError::Usage(
            "configure a model with --persona openai-compatible --persona-url <url> --persona-model <model>; --persona fake explicitly selects a test fixture".to_owned()
        ));
    }
    // Real conversations default to local-only. A wider destination requires
    // an explicit per-invocation privacy choice, including on subsequent runs.
    let privacy = options.privacy.unwrap_or(PrivacyConstraint::LocalOnly);
    config.persona.check_privacy(privacy)?;
    config.build_persona()?;
    config.persona_seed()?;
    if let Some(mio) = &config.mio {
        mio.validate()?;
    }
    if options.persona.is_some() || options.mio_options_present() || env_persona_changed {
        runtime.save_config(config)?;
    }
    let mut session = DialogueSession::start(
        &mut runtime,
        options.subject.as_deref().unwrap_or("local-user"),
        privacy,
    )?;
    session.set_debug_context(options.debug_context);
    session.set_debug_trace(options.debug);
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let result = if let Some(message) = &options.message {
        session
            .turn(&mut runtime, message, |reply| {
                serde_json::to_writer(&mut output, reply)?;
                writeln!(output)?;
                output.flush()
            })
            .map(|_| ())
    } else {
        let stdin = std::io::stdin();
        let terminal = stdin.is_terminal();
        let mut input = stdin.lock();
        kamimusuhi_runtime::c0::chat::run_repl(
            &mut runtime,
            &mut session,
            &mut input,
            &mut output,
            terminal,
            options.debug_context,
            options.debug,
        )
    };
    runtime.stopping();
    result?;
    Ok(String::new())
}

/// Standalone derived-lane commands: `reflect` and `activate` read + decide;
/// `rollback` moves the head back. None touches canonical history.
fn run_c0_command(command: &str, options: &Options) -> Result<String, RuntimeError> {
    let mut runtime = Runtime::open(options.dir()?, options.runtime_options())?;
    let subject = options.subject.as_deref().unwrap_or("local-user");
    match command {
        "reflect" => {
            let mut writer = None;
            let report = kamimusuhi_runtime::c0::reflection::run(
                &mut runtime,
                subject,
                None,
                None,
                &mut writer,
            )?;
            runtime.stopping();
            encode(&report)
        }
        "rollback" => {
            let outcome = kamimusuhi_runtime::c0::rollback(&runtime, None, None)?;
            runtime.stopping();
            encode(&serde_json::json!({
                "rolled_back_seq": outcome.rolled_back_seq,
                "restored_seq": outcome.restored_seq,
                "proposal_id": outcome.proposal_id,
            }))
        }
        "activate" => {
            let raw = options.proposal_id.as_deref().ok_or_else(|| {
                RuntimeError::Usage("activate requires --proposal-id <hex>".to_owned())
            })?;
            let proposal_id: kamimusuhi_core::ids::C0ProposalId = raw.parse().map_err(|_| {
                RuntimeError::Usage("--proposal-id must be a 32-char hex id".to_owned())
            })?;
            let proposal = runtime
                .store()
                .c0_proposal(proposal_id)?
                .ok_or_else(|| RuntimeError::Usage("proposal not found".to_owned()))?;
            let report =
                kamimusuhi_runtime::c0::replay::replay_proposal(&runtime, subject, &proposal)?;
            let decision = kamimusuhi_core::c0::gate_decision(&proposal, &report);
            match decision {
                kamimusuhi_core::c0::GateDecision::Accept => {
                    let activation =
                        kamimusuhi_runtime::c0::activate(&runtime, proposal_id, None, None)?;
                    runtime.stopping();
                    encode(&serde_json::json!({
                        "gate": "accept",
                        "activation_seq": activation.activation_seq,
                        "replay": report,
                    }))
                }
                kamimusuhi_core::c0::GateDecision::Reject { reason } => {
                    runtime.store().c0_decide(
                        proposal_id,
                        kamimusuhi_core::c0::ProposalStatus::Rejected,
                        Some(&reason),
                        runtime.now(),
                    )?;
                    runtime.stopping();
                    encode(&serde_json::json!({
                        "gate": "reject",
                        "reason": reason,
                        "replay": report,
                    }))
                }
            }
        }
        _ => unreachable!(),
    }
}

fn encode<T: serde::Serialize>(value: &T) -> Result<String, RuntimeError> {
    serde_json::to_string_pretty(value)
        .map_err(|source| RuntimeError::Usage(format!("could not encode report: {source}")))
}

#[derive(Debug, Default)]
struct Options {
    dir: Option<String>,
    source_root: Option<String>,
    message: Option<String>,
    subject: Option<String>,
    mio_url: Option<String>,
    mio_experiment: Option<String>,
    mio_genome: Option<String>,
    mio_max_age_secs: Option<u64>,
    no_mio: bool,
    phase: Option<DemoPhase>,
    resource: Option<ResourceImplementation>,
    id_seed: Option<u64>,
    clock: Option<ClockMode>,
    privacy: Option<PrivacyConstraint>,
    urgency: Option<Urgency>,
    persona: Option<PersonaBackendKind>,
    persona_url: Option<String>,
    persona_model: Option<String>,
    persona_auth_env: Option<String>,
    persona_locality: Option<LocalityClass>,
    debug_context: bool,
    debug: bool,
    proposal_id: Option<String>,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, RuntimeError> {
        let mut options = Self::default();
        let mut args = args.peekable();
        while let Some(flag) = args.next() {
            let mut value = || {
                args.next().ok_or_else(|| {
                    RuntimeError::Usage(format!("{flag} requires a value\n\n{USAGE}"))
                })
            };
            match flag.as_str() {
                "--dir" => options.dir = Some(value()?),
                "--source-root" => options.source_root = Some(value()?),
                "--message" => options.message = Some(value()?),
                "--subject" => options.subject = Some(value()?),
                "--mio-url" => options.mio_url = Some(value()?),
                "--mio-experiment" => options.mio_experiment = Some(value()?),
                "--mio-genome" => options.mio_genome = Some(value()?),
                "--mio-max-age-secs" => {
                    options.mio_max_age_secs = Some(value()?.parse().map_err(|_| {
                        RuntimeError::Usage(
                            "MIO max age must be seconds between 1 and 86400".to_owned(),
                        )
                    })?);
                }
                "--no-mio" => options.no_mio = true,
                "--phase" => options.phase = Some(value()?.parse()?),
                "--resource" => {
                    let raw = value()?;
                    options.resource = Some(raw.parse().map_err(|_| {
                        RuntimeError::Usage(format!(
                            "unknown --resource {raw:?}; expected fake-a, fake-b or fake-unavailable"
                        ))
                    })?);
                }
                "--seed" => {
                    let raw = value()?;
                    options.id_seed = Some(raw.parse().map_err(|_| {
                        RuntimeError::Usage(format!("--seed {raw:?} is not a number"))
                    })?);
                    options.clock = Some(ClockMode::Fixed);
                }
                "--id-seed" => {
                    let raw = value()?;
                    options.id_seed = Some(raw.parse().map_err(|_| {
                        RuntimeError::Usage(format!("--id-seed {raw:?} is not a number"))
                    })?);
                }
                "--clock" => options.clock = Some(value()?.parse()?),
                "--persona" => {
                    let raw = value()?;
                    options.persona = Some(raw.parse().map_err(|_| {
                        RuntimeError::Usage(format!(
                            "unknown --persona {raw:?}; expected fake or openai-compatible"
                        ))
                    })?);
                }
                "--persona-url" => options.persona_url = Some(value()?),
                "--persona-model" => options.persona_model = Some(value()?),
                "--persona-auth-env" => {
                    let name = value()?;
                    if name.is_empty()
                        || !name.bytes().enumerate().all(|(index, byte)| {
                            byte == b'_'
                                || byte.is_ascii_alphabetic()
                                || (index > 0 && byte.is_ascii_digit())
                        })
                    {
                        return Err(RuntimeError::Usage(
                            "--persona-auth-env requires an environment variable name, not an API key".to_owned()
                        ));
                    }
                    options.persona_auth_env = Some(name);
                }
                "--persona-locality" => {
                    let raw = value()?;
                    options.persona_locality = Some(raw.replace('-', "_").parse().map_err(|_| {
                        RuntimeError::Usage("invalid --persona-locality; expected local-host, local-network or external".to_owned())
                    })?);
                }
                "--debug-context" => options.debug_context = true,
                "--debug" => options.debug = true,
                "--proposal-id" => options.proposal_id = Some(value()?),
                "--privacy" => {
                    let raw = value()?;
                    // Hyphens on the command line, underscores on the wire.
                    options.privacy = Some(raw.replace('-', "_").parse().map_err(|_| {
                        RuntimeError::Usage(format!(
                            "unknown --privacy {raw:?}; expected local-only, \
                             no-external-service or unconstrained"
                        ))
                    })?);
                }
                "--urgency" => {
                    let raw = value()?;
                    options.urgency = Some(raw.parse().map_err(|_| {
                        RuntimeError::Usage(format!(
                            "unknown --urgency {raw:?}; expected interactive or background"
                        ))
                    })?);
                }
                other => {
                    return Err(RuntimeError::Usage(format!(
                        "unknown option {other:?}\n\n{USAGE}"
                    )));
                }
            }
        }
        Ok(options)
    }

    fn dir(&self) -> Result<&str, RuntimeError> {
        self.dir
            .as_deref()
            .ok_or_else(|| RuntimeError::Usage(format!("--dir is required\n\n{USAGE}")))
    }

    fn mio_options_present(&self) -> bool {
        self.mio_url.is_some()
            || self.mio_experiment.is_some()
            || self.mio_genome.is_some()
            || self.mio_max_age_secs.is_some()
            || self.no_mio
    }

    fn mio_setting(
        &self,
        existing: Option<&MioBinding>,
    ) -> Result<Option<MioBinding>, RuntimeError> {
        if self.no_mio {
            if self.mio_url.is_some()
                || self.mio_experiment.is_some()
                || self.mio_genome.is_some()
                || self.mio_max_age_secs.is_some()
            {
                return Err(RuntimeError::Usage(
                    "--no-mio cannot be combined with MIO settings".to_owned(),
                ));
            }
            return Ok(None);
        }
        let mut binding = match (&self.mio_url, &self.mio_experiment, &self.mio_genome) {
            (Some(url), Some(experiment), Some(genome)) => {
                MioBinding::new(url.clone(), experiment.clone(), genome.clone())
            }
            (None, None, None) => existing
                .cloned()
                .ok_or_else(|| RuntimeError::Usage("no MIO binding to update".to_owned()))?,
            _ => {
                return Err(RuntimeError::Usage(
                    "set --mio-url, --mio-experiment and --mio-genome together".to_owned(),
                ));
            }
        };
        if let Some(seconds) = self.mio_max_age_secs {
            binding.max_age_ms = seconds
                .checked_mul(1000)
                .ok_or_else(|| RuntimeError::Usage("MIO max age is out of range".to_owned()))?;
        }
        binding.validate()?;
        Ok(Some(binding))
    }

    /// Build the persona namespace from the flags, keeping whatever the file
    /// already declared for anything not given on the command line.
    fn persona_setting(
        &self,
        backend: PersonaBackendKind,
        config: &kamimusuhi_runtime::RuntimeConfig,
    ) -> Result<PersonaSetting, RuntimeError> {
        match backend {
            PersonaBackendKind::Fake => Ok(PersonaSetting {
                backend,
                provider: config.persona.provider.clone(),
                // Switching backend on the command line must not change the
                // disposition: the seed belongs to the individual, not to
                // whatever is currently running it.
                seed: config.persona.seed.clone(),
            }),
            PersonaBackendKind::OpenaiCompatible => {
                let existing = config.persona.provider.clone();
                let base_url = self
                    .persona_url
                    .clone()
                    .or_else(|| existing.as_ref().map(|p| p.base_url.clone()))
                    .ok_or_else(|| {
                        RuntimeError::Usage(
                            "--persona openai-compatible needs --persona-url, or a                              persona.provider entry in runtime.json"
                                .to_owned(),
                        )
                    })?;
                let model = self
                    .persona_model
                    .clone()
                    .or_else(|| existing.as_ref().map(|p| p.model.clone()))
                    .ok_or_else(|| {
                        RuntimeError::Usage(
                            "--persona openai-compatible needs --persona-model, or a                              persona.provider entry in runtime.json"
                                .to_owned(),
                        )
                    })?;
                // A new endpoint is a new data destination: do not inherit
                // the previous endpoint's locality declaration implicitly.
                let same_endpoint = existing.as_ref().filter(|p| p.base_url == base_url);
                let same_backend = same_endpoint.filter(|p| p.model == model);
                Ok(PersonaSetting {
                    backend,
                    seed: config.persona.seed.clone(),
                    provider: Some(PersonaProviderConfig {
                        // A stable ID per configured endpoint, derived from
                        // what identifies it, so the same endpoint keeps the
                        // same attribution across runs.
                        backend_id: same_backend
                            .map(|p| p.backend_id)
                            .unwrap_or_else(|| persona_backend_id_for(&base_url, &model)),
                        locality: self.persona_locality.unwrap_or_else(|| {
                            same_endpoint.map_or(LocalityClass::External, |p| p.locality)
                        }),
                        base_url,
                        model,
                        // Never forward an old endpoint's credential to a
                        // new destination just because --persona-url changed.
                        auth_env: self
                            .persona_auth_env
                            .clone()
                            .or_else(|| same_endpoint.and_then(|p| p.auth_env.clone())),
                        timeout_ms: existing.as_ref().map_or(60_000, |p| p.timeout_ms),
                        tls_root_ca_path: same_endpoint.and_then(|p| p.tls_root_ca_path.clone()),
                        system_instruction: existing
                            .as_ref()
                            .and_then(|p| p.system_instruction.clone()),
                    }),
                })
            }
        }
    }

    fn runtime_options(&self) -> RuntimeOptions {
        RuntimeOptions {
            id_seed: self.id_seed,
            clock: self.clock.unwrap_or_default(),
        }
    }
}
