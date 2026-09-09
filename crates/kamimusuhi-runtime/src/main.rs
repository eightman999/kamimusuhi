//! Kamimusuhi runtime binary.
//!
//! ```text
//! kamimusuhi-runtime init            --dir <path> [--resource <impl>] [--seed <n>]
//! kamimusuhi-runtime inspect         --dir <path> [--seed <n>]
//! kamimusuhi-runtime demo-continuity --dir <path> --phase first|resume
//!                                    [--resource <impl>] [--seed <n>]
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

use std::process::ExitCode;

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::routing::{LocalityClass, PrivacyConstraint};
use kamimusuhi_runtime::config::GENERAL_SLOT;
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

const USAGE: &str = "\
kamimusuhi-runtime <command> [options]

commands:
  init              prepare a runtime directory; creates an individual only
                    when the canonical database holds none
  inspect           read-only report of identity, lineage, memory, Library
                    and resource calls
  demo-continuity   run one phase of the deterministic restart scenario

options:
  --dir <path>      runtime directory (required)
  --phase <p>       demo-continuity only: first | resume
  --resource <i>    implementation filling the general slot: fake-a |
                    fake-b | fake-unavailable | openai-compatible
  --id-seed <n>     deterministic ID sequence
  --clock <c>       system | fixed
  --seed <n>        shorthand for --id-seed <n> --clock fixed
  --privacy <p>     how far this turn's material may travel:
                    local-only | no-external-service | unconstrained
  --persona <p>     Persona Core backend: fake | openai-compatible.
                    A model-backed persona needs a `persona.provider` entry in
                    runtime.json; the Persona namespace is separate from
                    `resources` and is never offered to the router.
  --persona-url <u> base URL of the persona endpoint, e.g.
                    http://127.0.0.1:11434/v1
  --persona-model <m>  model name to ask the persona endpoint for
  --persona-locality <l>  local-host | local-network | external (default: external)
";

fn main() -> ExitCode {
    match run() {
        Ok(output) => {
            println!("{output}");
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

    match command.as_str() {
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
                },
            )?;
            runtime.stopping();
            encode(&report)
        }
        "--help" | "-h" | "help" => Ok(USAGE.to_owned()),
        other => Err(RuntimeError::Usage(format!(
            "unknown command {other:?}\n\n{USAGE}"
        ))),
    }
}

fn encode<T: serde::Serialize>(value: &T) -> Result<String, RuntimeError> {
    serde_json::to_string_pretty(value)
        .map_err(|source| RuntimeError::Usage(format!("could not encode report: {source}")))
}

#[derive(Debug, Default)]
struct Options {
    dir: Option<String>,
    phase: Option<DemoPhase>,
    resource: Option<ResourceImplementation>,
    id_seed: Option<u64>,
    clock: Option<ClockMode>,
    privacy: Option<PrivacyConstraint>,
    persona: Option<PersonaBackendKind>,
    persona_url: Option<String>,
    persona_model: Option<String>,
    persona_locality: Option<LocalityClass>,
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
                "--persona-locality" => {
                    let raw = value()?;
                    options.persona_locality = Some(raw.replace('-', "_").parse().map_err(|_| {
                        RuntimeError::Usage("invalid --persona-locality; expected local-host, local-network or external".to_owned())
                    })?);
                }
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
                        auth_env: same_endpoint.and_then(|p| p.auth_env.clone()),
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
