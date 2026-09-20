//! Opt-in, single-turn live smoke without reading or writing user history.
use std::time::Instant;

use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::dialogue_setup::{
    environment_value_available, language_provider_auth_available,
    persona_setting_from_environment, register_hai_language_providers,
};
use kamimusuhi_runtime::llm_jev::{HAI_API_KEY_ENV, TYPESAFE_API_KEY_ENV};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() != Some("--live") {
        return Err("pass --live to send one greeting to the configured primary organ, both HAI models and Jev".into());
    }
    for name in [TYPESAFE_API_KEY_ENV, HAI_API_KEY_ENV] {
        if !environment_value_available(name) {
            return Err(
                format!("required credential environment variable {name} is unavailable").into(),
            );
        }
    }
    let dir = tempfile::tempdir()?;
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::default(),
        ResourceImplementation::FakeA,
    )?;
    let mut config = runtime.config().clone();
    config.persona = persona_setting_from_environment(&config)?
        .ok_or("configure KAMIMUSUHI_LLM_PROVIDER and its endpoint/model")?;
    if config
        .persona
        .provider
        .as_ref()
        .is_some_and(|provider| !language_provider_auth_available(provider))
    {
        return Err("configured primary credential is unavailable".into());
    }
    register_hai_language_providers(&mut config)?;
    runtime.save_config(config)?;
    let mut session = DialogueSession::start(
        &mut runtime,
        "parallel-smoke",
        PrivacyConstraint::Unconstrained,
    )?;
    session.set_debug_trace(true);
    let started = Instant::now();
    let result = session.turn(
        &mut runtime,
        "こんにちは。短くひと言で挨拶してください。",
        |_| Ok(()),
    );
    // Metadata only: no credentials, history, prompts or candidate text.
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "elapsed_ms": started.elapsed().as_millis(),
            "generation_latency_ms": session.last_generation_latency_ms(),
            "generated_candidates": session.last_generated_candidates(),
            "trace": result.as_ref().ok().and_then(|reply| reply.llm_jev.as_ref()),
            "success": result.is_ok(),
        }))?
    );
    runtime.stopping();
    result?;
    Ok(())
}
