use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use kamimusuhi_core::persona::{ConversationMessage, ConversationRole};
use kamimusuhi_core::routing::{LocalityClass, PrivacyConstraint};
use kamimusuhi_runtime::config::validate_language_provider_id;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::dialogue_setup::{
    language_provider_auth_available, language_provider_kind, persona_backend_id_for,
    persona_setting_from_environment, public_endpoint_origin, register_hai_language_providers,
};
use kamimusuhi_runtime::llm_jev::{
    ConversationCoreState, ConversationTrace, GeneratedLanguageCandidate, LLM_MODEL_ENV,
    LLM_PROVIDER_ENV, LanguageProviderTelemetry, LanguageProviderTelemetrySnapshot, TypesafeConfig,
};
use kamimusuhi_runtime::{
    PersonaProviderConfig, ResourceImplementation, Runtime, RuntimeError, RuntimeOptions,
};

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub runtime_dir: PathBuf,
    pub subject: String,
    pub privacy: PrivacyConstraint,
}

pub enum WorkerCommand {
    Send {
        request_id: u64,
        text: String,
    },
    AddLanguageProvider {
        id: String,
        base_url: String,
        model: String,
        auth_env: Option<String>,
    },
    RemoveLanguageProvider {
        id: String,
    },
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct DisplayMessage {
    pub role: ConversationRole,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct LanguageProviderSummary {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub auth_env: Option<String>,
    pub active: bool,
    pub telemetry: LanguageProviderTelemetrySnapshot,
}

#[derive(Debug, Clone)]
pub struct ConnectionSummary {
    pub jev_configured: bool,
    pub jev_model: String,
    pub jev_base_url: String,
    pub llm_provider: String,
    pub llm_model: String,
    pub llm_base_url: String,
    pub auth_variables: Vec<String>,
    pub language_providers: Vec<LanguageProviderSummary>,
}

#[derive(Debug, Clone)]
pub struct ReadyState {
    pub individual_id: String,
    pub session_id: String,
    pub subject: String,
    pub history: Vec<DisplayMessage>,
    pub connection: ConnectionSummary,
}

#[derive(Debug)]
pub enum WorkerEvent {
    Ready(ReadyState),
    GenerationReport {
        candidates: Vec<GeneratedLanguageCandidate>,
        elapsed_ms: u64,
    },
    Reply {
        response: String,
        trace: Option<Box<ConversationTrace>>,
        elapsed_ms: u128,
        history_messages: usize,
        core_state: Option<ConversationCoreState>,
    },
    Error {
        message: String,
    },
    ProvidersUpdated(Vec<LanguageProviderSummary>),
    Stopped,
}

pub fn spawn(config: WorkerConfig) -> (Sender<WorkerCommand>, Receiver<WorkerEvent>) {
    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("kamimusuhi-dialogue".to_owned())
        .spawn(move || run(config, command_rx, event_tx))
        .expect("native dialogue worker thread must start");
    (command_tx, event_rx)
}

fn run(config: WorkerConfig, commands: Receiver<WorkerCommand>, events: Sender<WorkerEvent>) {
    let setup = initialize(&config);
    let (mut runtime, mut session, history, connection) = match setup {
        Ok(value) => value,
        Err(error) => {
            let _ = events.send(WorkerEvent::Error { message: error });
            return;
        }
    };

    let ready = ReadyState {
        individual_id: runtime.individual_id().to_string(),
        session_id: session.session_id().to_string(),
        subject: session.subject().to_owned(),
        history,
        connection,
    };
    if events.send(WorkerEvent::Ready(ready)).is_err() {
        runtime.stopping();
        return;
    }

    let mut accepted_requests = HashSet::new();
    while let Ok(command) = commands.recv() {
        match command {
            WorkerCommand::Shutdown => break,
            WorkerCommand::Send { request_id, text } => {
                if !accepted_requests.insert(request_id) {
                    let _ = events.send(WorkerEvent::Error {
                        message: "同じ送信IDは二重実行しません。画面から再送してください。"
                            .to_owned(),
                    });
                    continue;
                }
                if accepted_requests.len() > 256 {
                    accepted_requests.clear();
                    accepted_requests.insert(request_id);
                }
                let started = Instant::now();
                let result = session.turn(&mut runtime, &text, |_| Ok(()));
                let elapsed_ms = started.elapsed().as_millis();
                let _ = events.send(WorkerEvent::GenerationReport {
                    candidates: session.last_generated_candidates().to_vec(),
                    elapsed_ms: session.last_generation_latency_ms(),
                });
                match result {
                    Ok(reply) => {
                        let core_state = reply
                            .llm_jev
                            .as_ref()
                            .map(|trace| trace.core_state_after.clone());
                        let _ = events.send(WorkerEvent::Reply {
                            response: reply.response,
                            trace: reply.llm_jev.map(Box::new),
                            elapsed_ms,
                            history_messages: reply.history_messages,
                            core_state,
                        });
                        let _ = events.send(WorkerEvent::ProvidersUpdated(
                            language_provider_summaries(runtime.config(), config.privacy, &session),
                        ));
                    }
                    Err(error) => {
                        let _ = events.send(WorkerEvent::ProvidersUpdated(
                            language_provider_summaries(runtime.config(), config.privacy, &session),
                        ));
                        let _ = events.send(WorkerEvent::Error {
                            message: error.to_string(),
                        });
                    }
                }
            }
            WorkerCommand::AddLanguageProvider {
                id,
                base_url,
                model,
                auth_env,
            } => match add_language_provider(
                &mut runtime,
                &mut session,
                config.privacy,
                &id,
                &base_url,
                &model,
                auth_env.as_deref(),
            ) {
                Ok(providers) => {
                    let _ = events.send(WorkerEvent::ProvidersUpdated(providers));
                }
                Err(message) => {
                    let _ = events.send(WorkerEvent::Error { message });
                }
            },
            WorkerCommand::RemoveLanguageProvider { id } => {
                match remove_language_provider(&mut runtime, &mut session, config.privacy, &id) {
                    Ok(providers) => {
                        let _ = events.send(WorkerEvent::ProvidersUpdated(providers));
                    }
                    Err(message) => {
                        let _ = events.send(WorkerEvent::Error { message });
                    }
                }
            }
        }
    }
    runtime.stopping();
    let _ = events.send(WorkerEvent::Stopped);
}

fn add_language_provider(
    runtime: &mut Runtime,
    session: &mut DialogueSession,
    privacy: PrivacyConstraint,
    id: &str,
    base_url: &str,
    model: &str,
    auth_env: Option<&str>,
) -> Result<Vec<LanguageProviderSummary>, String> {
    let provider = make_language_provider_config(id, base_url, model, auth_env)
        .map_err(format_runtime_error)?;
    let mut next = runtime.config().clone();
    next.set_language_provider(id, provider.clone())
        .map_err(format_runtime_error)?;
    session
        .set_language_provider(id, &provider)
        .map_err(format_runtime_error)?;
    runtime.save_config(next).map_err(format_runtime_error)?;
    Ok(language_provider_summaries(
        runtime.config(),
        privacy,
        session,
    ))
}

fn remove_language_provider(
    runtime: &mut Runtime,
    session: &mut DialogueSession,
    privacy: PrivacyConstraint,
    id: &str,
) -> Result<Vec<LanguageProviderSummary>, String> {
    if !runtime.config().language_providers.contains_key(id) {
        return Err(format!("language provider {id:?} is not registered"));
    }
    let mut next = runtime.config().clone();
    next.remove_language_provider(id);
    session
        .remove_language_provider(id)
        .map_err(format_runtime_error)?;
    runtime.save_config(next).map_err(format_runtime_error)?;
    Ok(language_provider_summaries(
        runtime.config(),
        privacy,
        session,
    ))
}

fn make_language_provider_config(
    id: &str,
    base_url: &str,
    model: &str,
    auth_env: Option<&str>,
) -> Result<PersonaProviderConfig, RuntimeError> {
    validate_language_provider_id(id)?;
    if base_url.trim().is_empty() || model.trim().is_empty() {
        return Err(RuntimeError::Usage(
            "追加LLM APIにはbase URLとmodelが必要です".to_owned(),
        ));
    }
    let auth_env = auth_env
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if let Some(name) = &auth_env {
        validate_environment_name(name)?;
    }
    Ok(PersonaProviderConfig {
        backend_id: persona_backend_id_for(base_url.trim(), model.trim()),
        locality: LocalityClass::External,
        base_url: base_url.trim().to_owned(),
        model: model.trim().to_owned(),
        auth_env,
        timeout_ms: 60_000,
        tls_root_ca_path: None,
        system_instruction: None,
        reasoning: Default::default(),
        extra_body: None,
    })
}

fn validate_environment_name(name: &str) -> Result<(), RuntimeError> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(RuntimeError::Usage(
            "認証環境変数名はASCII英数字と '_' のみ指定できます".to_owned(),
        ));
    }
    Ok(())
}

fn language_provider_summaries(
    config: &kamimusuhi_runtime::RuntimeConfig,
    privacy: PrivacyConstraint,
    session: &DialogueSession,
) -> Vec<LanguageProviderSummary> {
    let telemetry = session.language_provider_telemetry();
    config
        .language_providers
        .iter()
        .map(|(id, provider)| LanguageProviderSummary {
            id: id.clone(),
            provider: language_provider_kind(provider).to_owned(),
            model: provider.model.clone(),
            base_url: public_endpoint_origin(&provider.base_url),
            auth_env: provider.auth_env.clone(),
            active: privacy.admits(provider.locality) && language_provider_auth_available(provider),
            telemetry: telemetry
                .get(id)
                .cloned()
                .unwrap_or_else(|| LanguageProviderTelemetry::default().snapshot()),
        })
        .collect()
}

fn initialize(
    config: &WorkerConfig,
) -> Result<
    (
        Runtime,
        DialogueSession,
        Vec<DisplayMessage>,
        ConnectionSummary,
    ),
    String,
> {
    let mut runtime = if config.runtime_dir.join("runtime.json").exists() {
        Runtime::open(&config.runtime_dir, RuntimeOptions::default())
    } else {
        Runtime::init(
            &config.runtime_dir,
            RuntimeOptions::default(),
            ResourceImplementation::FakeA,
        )
    }
    .map_err(format_runtime_error)?;

    if let Some(setting) =
        persona_setting_from_environment(runtime.config()).map_err(format_runtime_error)?
        && runtime.config().persona != setting
    {
        let mut next = runtime.config().clone();
        next.persona = setting;
        runtime.save_config(next).map_err(format_runtime_error)?;
    }

    let mut next = runtime.config().clone();
    if register_hai_language_providers(&mut next).map_err(format_runtime_error)? {
        runtime.save_config(next).map_err(format_runtime_error)?;
    }

    runtime
        .config()
        .persona
        .check_privacy(config.privacy)
        .map_err(format_runtime_error)?;
    runtime
        .config()
        .build_persona()
        .map_err(format_runtime_error)?;
    runtime
        .config()
        .persona_seed()
        .map_err(format_runtime_error)?;

    let mut session = DialogueSession::start(&mut runtime, &config.subject, config.privacy)
        .map_err(format_runtime_error)?;
    session.set_debug_trace(true);
    let history = session
        .history_for_display(&runtime)
        .map_err(format_runtime_error)?
        .into_iter()
        .map(display_message)
        .collect();
    let connection = connection_summary(runtime.config(), config.privacy, &session);
    Ok((runtime, session, history, connection))
}

fn display_message(message: ConversationMessage) -> DisplayMessage {
    DisplayMessage {
        role: message.role,
        text: message.text,
    }
}

fn connection_summary(
    config: &kamimusuhi_runtime::RuntimeConfig,
    privacy: PrivacyConstraint,
    session: &DialogueSession,
) -> ConnectionSummary {
    let jev = TypesafeConfig::from_env();
    let (llm_provider, llm_model, llm_base_url, llm_auth) = match config.persona.provider.as_ref() {
        Some(provider) => (
            std::env::var(LLM_PROVIDER_ENV).unwrap_or_else(|_| "openai-compatible".to_owned()),
            provider.model.clone(),
            provider.base_url.clone(),
            provider.auth_env.clone(),
        ),
        None => (
            std::env::var(LLM_PROVIDER_ENV).unwrap_or_else(|_| "in-process".to_owned()),
            std::env::var(LLM_MODEL_ENV).unwrap_or_else(|_| "fake-persona".to_owned()),
            "in-process".to_owned(),
            None,
        ),
    };
    let mut auth_variables = Vec::new();
    if jev.is_some() {
        auth_variables.push("TYPESAFE_API_KEY".to_owned());
    }
    if let Some(auth) = llm_auth {
        auth_variables.push(auth);
    }
    let language_providers = language_provider_summaries(config, privacy, session);
    for provider in &language_providers {
        if let Some(auth) = &provider.auth_env
            && !auth_variables.iter().any(|current| current == auth)
        {
            auth_variables.push(auth.clone());
        }
    }
    ConnectionSummary {
        jev_configured: jev.is_some(),
        jev_model: jev
            .as_ref()
            .map_or_else(|| "未設定".to_owned(), |value| value.model.clone()),
        jev_base_url: jev.as_ref().map_or_else(
            || safe_origin("https://api.typesafe.ai").to_owned(),
            |value| safe_origin(&value.base_url),
        ),
        llm_provider,
        llm_model,
        llm_base_url: safe_origin(&llm_base_url),
        auth_variables,
        language_providers,
    }
}

fn safe_origin(raw: &str) -> String {
    public_endpoint_origin(raw)
}

fn format_runtime_error(error: RuntimeError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_message_preserves_roles_and_text() {
        let message = display_message(ConversationMessage {
            evidence_id: "00000000000000000000000000000000".parse().unwrap(),
            role: ConversationRole::User,
            text: "こんにちは".to_owned(),
        });
        assert_eq!(message.role, ConversationRole::User);
        assert_eq!(message.text, "こんにちは");
    }

    #[test]
    fn max_input_is_defined_by_the_runtime_contract() {
        assert_eq!(kamimusuhi_runtime::dialogue::MAX_INPUT_BYTES, 8_192);
    }

    #[test]
    fn safe_origin_does_not_expose_url_credentials_or_paths() {
        assert_eq!(
            safe_origin("https://secret:token@example.test/v1?key=hidden"),
            "https://example.test"
        );
    }

    #[test]
    fn additional_provider_config_keeps_only_the_auth_environment_name() {
        let config = make_language_provider_config(
            "backup",
            "https://provider.example/v1",
            "backup-model",
            Some("BACKUP_API_KEY"),
        )
        .unwrap();
        assert_eq!(config.model, "backup-model");
        assert_eq!(config.auth_env.as_deref(), Some("BACKUP_API_KEY"));
        assert_eq!(config.locality, LocalityClass::External);
    }

    #[test]
    fn additional_provider_rejects_secret_like_environment_name() {
        let error = make_language_provider_config(
            "backup",
            "https://provider.example/v1",
            "backup-model",
            Some("actual-secret-value"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("環境変数名"));
        assert!(!error.to_string().contains("actual-secret-value"));
    }
}
