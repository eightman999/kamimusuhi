//! K-CORE acceptance tests. All backends are fixtures; no checkpoint/API/GPU.
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use kamimusuhi_core::organs::{
    CognitiveOrgan, OrganDescriptor, OrganError, OrganInput, OrganSignal, PromotionMode,
};
use kamimusuhi_runtime::kcore::{CoreIntent, KCore, KCoreConfig, PerceptFrame, ProcessBinding};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions, validated_experiment_manifest};
use serde_json::{Value, json};

fn init() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    Runtime::init(
        dir.path(),
        RuntimeOptions::default(),
        ResourceImplementation::FakeUnavailable,
    )
    .unwrap();
    dir
}

fn frame(sequence: u64) -> PerceptFrame {
    PerceptFrame {
        source: "fixture".to_owned(),
        sequence,
        observation: vec![0.2, 0.3],
        quality_milli: 1_000,
        age_ms: 0,
    }
}

fn command(dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_k-core"));
    cmd.arg("--dir").arg(dir).args(["--interval-ms", "1"]);
    cmd
}

fn cli(dir: &std::path::Path, frames: Option<&str>) -> Vec<Value> {
    let mut cmd = command(dir);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    if frames.is_some() {
        cmd.arg("--stdin").stdin(Stdio::piped());
    } else {
        cmd.args(["--ticks", "3"]);
    }
    let mut child = cmd.spawn().unwrap();
    if let Some(frames) = frames {
        child.stdin.take().unwrap().write_all(frames.as_bytes()).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn cli_restart_preserves_canonical_state_without_models() {
    let dir = init();
    let first = cli(dir.path(), None);
    let second = cli(dir.path(), None);
    assert_eq!(first[0]["canonical"], first.last().unwrap()["canonical"]);
    assert_eq!(first[0]["canonical"], second[0]["canonical"]);
    assert_eq!(first.last().unwrap()["ticks"], 3);
    assert_eq!(first[1]["report"]["individual_id"], second[1]["report"]["individual_id"]);
    assert_ne!(first[1]["report"]["boot_id"], second[1]["report"]["boot_id"]);
    assert_eq!(second[1]["report"]["tick"], 1);
}

#[test]
fn jsonl_drains_at_eof_and_rejects_replay_without_mutations() {
    let dir = init();
    let input = [1, 1, 2].into_iter()
        .map(|sequence| serde_json::to_string(&frame(sequence)).unwrap() + "\n")
        .collect::<String>();
    let rows = cli(dir.path(), Some(&input));
    let rejected = rows.iter().filter(|row| row["event"] == "ingress_rejected").count();
    assert_eq!(rejected, 1);
    let processed = rows.iter().filter(|row| row["report"]["sequence"].is_number()).count();
    assert_eq!(processed, 2);
    assert_eq!(rows[0]["canonical"], rows.last().unwrap()["canonical"]);
    assert!(rows.iter().filter(|row| row["event"] == "tick")
        .all(|row| row["report"]["authorizes_mutation"] == false));
}

#[test]
fn cli_refuses_missing_runtime_and_invalid_input() {
    let missing = tempfile::tempdir().unwrap();
    assert!(!command(missing.path()).args(["--ticks", "1"]).output().unwrap().status.success());
    assert!(!missing.path().join("kamimusuhi.sqlite").exists());
    let dir = init();
    let mut child = command(dir.path()).arg("--stdin")
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"{\"evidence_refs\":[\"SECRET_FIXTURE\"]}\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET_FIXTURE"));
    assert!(KCore::open(dir.path(), RuntimeOptions::default(), KCoreConfig::default()).is_ok());
}

#[test]
fn killed_process_releases_the_loop_lock_without_recreating_identity() {
    let dir = init();
    let mut child = command(dir.path()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut boot = String::new();
    reader.read_line(&mut boot).unwrap();
    let boot: Value = serde_json::from_str(&boot).unwrap();
    let blocked = command(dir.path()).args(["--ticks", "1"]).output().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!blocked.status.success());
    let restarted = cli(dir.path(), None);
    assert_eq!(boot["canonical"], restarted[0]["canonical"]);
}

struct FixtureOrgan {
    descriptor: OrganDescriptor,
    fail: bool,
    delay: Duration,
}

impl CognitiveOrgan for FixtureOrgan {
    fn descriptor(&self) -> OrganDescriptor {
        self.descriptor.clone()
    }

    fn process(&mut self, input: &OrganInput) -> Result<OrganSignal, OrganError> {
        std::thread::sleep(self.delay);
        if self.fail {
            return Err(OrganError::Backend { code: "FIXTURE".to_owned() });
        }
        Ok(OrganSignal {
            descriptor: self.descriptor.clone(),
            produced_at: input.observed_at,
            ttl_ms: 1_000,
            confidence_milli: None,
            payload: json!({"fixture": true}),
            evidence_refs: Vec::new(),
        })
    }
}

fn organ(index: usize, promotion: PromotionMode, fail: bool, delay: Duration) -> Box<dyn CognitiveOrgan> {
    let mut descriptor = validated_experiment_manifest()[index].clone();
    descriptor.promotion = promotion;
    Box::new(FixtureOrgan { descriptor, fail, delay })
}

#[test]
fn shadow_signals_and_failures_cannot_drive_live_policy() {
    let dir = init();
    let mut core = KCore::open(dir.path(), RuntimeOptions::default(), KCoreConfig::default()).unwrap();
    let before = core.inspect().unwrap();
    core.register(organ(0, PromotionMode::Shadow, false, Duration::ZERO)).unwrap();
    core.register(organ(1, PromotionMode::Shadow, true, Duration::ZERO)).unwrap();
    core.ingest(frame(1)).unwrap();
    let shadow = core.tick().unwrap();
    assert_eq!(shadow.intent, CoreIntent::Wait);
    assert_eq!(shadow.cycle.shadow.len(), 1);
    assert_eq!(shadow.cycle.failures.len(), 1);
    core.register(organ(2, PromotionMode::Active, false, Duration::ZERO)).unwrap();
    core.ingest(frame(2)).unwrap();
    assert_eq!(core.tick().unwrap().intent, CoreIntent::Observe);
    assert_eq!(before, core.inspect().unwrap());
    // Idle never reuses the previous successful signal.
    let idle = core.tick().unwrap();
    assert_eq!(idle.intent, CoreIntent::Wait);
    assert!(idle.cycle.active.is_empty());
}

#[test]
fn active_failure_or_expired_observation_falls_back_to_wait() {
    let dir = init();
    let mut core = KCore::open(dir.path(), RuntimeOptions::default(), KCoreConfig::default()).unwrap();
    core.register(organ(0, PromotionMode::Active, false, Duration::ZERO)).unwrap();
    core.register(organ(1, PromotionMode::Active, true, Duration::ZERO)).unwrap();
    core.ingest(frame(1)).unwrap();
    assert_eq!(core.tick().unwrap().intent, CoreIntent::Wait);
    drop(core);
    let config = KCoreConfig { max_age_ms: 100, ..KCoreConfig::default() };
    let mut core = KCore::open(dir.path(), RuntimeOptions::default(), config).unwrap();
    core.register(organ(0, PromotionMode::Active, false, Duration::from_millis(200))).unwrap();
    core.ingest(frame(1)).unwrap();
    let expired = core.tick().unwrap();
    assert_eq!(expired.intent, CoreIntent::Wait);
    assert_eq!(expired.discarded_stale, 1);
    assert!(expired.cycle.active.is_empty());
}

#[test]
fn optional_missing_backend_is_not_started_at_boot_and_fails_only_on_input() {
    let dir = init();
    let config = KCoreConfig {
        organs: vec![ProcessBinding {
            key: "h0-regulation".to_owned(),
            program: dir.path().join("nonexistent-fixture"),
            args: Vec::new(),
            promotion: PromotionMode::Active,
            timeout_ms: 100,
        }],
        ..KCoreConfig::default()
    };
    let mut core = KCore::open(dir.path(), RuntimeOptions::default(), config).unwrap();
    let before = core.inspect().unwrap();
    assert!(core.tick().unwrap().cycle.failures.is_empty());
    core.ingest(frame(1)).unwrap();
    let failed = core.tick().unwrap();
    assert_eq!(failed.intent, CoreIntent::Wait);
    assert_eq!(failed.cycle.failures.len(), 1);
    assert_eq!(before, core.inspect().unwrap());
}

#[test]
fn subprocess_state_is_ephemeral_and_does_not_become_evidence() {
    let dir = init();
    let script = dir.path().join("fixture.py");
    std::fs::write(&script, "import json, sys\nr=json.load(sys.stdin)\nn=(r.get('state') or {}).get('n',0)+1\nprint(json.dumps({'payload':{'n':n},'state':{'n':n},'ttl_ms':1000,'evidence_refs':['forged']}))\n").unwrap();
    let config = KCoreConfig {
        organs: vec![ProcessBinding {
            key: "h0-regulation".to_owned(),
            program: "python3".into(),
            args: vec![script.to_string_lossy().into_owned()],
            promotion: PromotionMode::Active,
            timeout_ms: 2_000,
        }],
        ..KCoreConfig::default()
    };
    let mut core = KCore::open(dir.path(), RuntimeOptions::default(), config.clone()).unwrap();
    let before = core.inspect().unwrap();
    for n in 1..=2 {
        core.ingest(frame(n)).unwrap();
        let tick = core.tick().unwrap();
        assert_eq!(tick.intent, CoreIntent::Observe);
        assert_eq!(tick.cycle.active[0].payload["n"], n);
        assert!(tick.cycle.active[0].evidence_refs.is_empty());
        assert!(!tick.cycle.active[0].authorizes_mutation());
    }
    assert_eq!(before, core.inspect().unwrap());
    drop(core);
    let mut core = KCore::open(dir.path(), RuntimeOptions::default(), config).unwrap();
    core.ingest(frame(1)).unwrap();
    assert_eq!(core.tick().unwrap().cycle.active[0].payload["n"], 1);
    assert_eq!(before, core.inspect().unwrap());
}
