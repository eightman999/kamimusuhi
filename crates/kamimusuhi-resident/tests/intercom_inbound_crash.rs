//! FIO write-path shape (a) on the intercom intake (see
//! `docs/verification/write-fault-shapes-2026-11-27.md`).
//!
//! `Intercom::accept` records the envelope id in `seen` together with the
//! envelope in the durable `inbound` queue, which is sound. The inbound
//! worker, however, pops the envelope *and persists the shorter queue*
//! before the dialogue turn runs. A crash while the turn is in flight
//! leaves an on-disk state in which the id is still `seen` (a resend is
//! answered `dedup`) but the envelope is gone from `inbound` — the message
//! that was acknowledged to the sender is never processed.
//!
//! The crash is simulated without killing anything: while the scripted
//! runtime is provably inside the turn, the persisted `intercom.json` is
//! copied as the crash image and a second resident is started from it.
//!
//! Fixed: the envelope now stays at the head of `inbound` until its turn
//! settles, with the start count kept in `in_flight`.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kamimusuhi_resident::config::Config;
use kamimusuhi_resident::intercom::{self, Envelope};
use kamimusuhi_resident::spool::Spool;
use kamimusuhi_resident::state::Shared;
use serde_json::{Value, json};

fn script(dir: &Path, body: &str) -> PathBuf {
    let bin = dir.join("fake-runtime");
    std::fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut perms = std::fs::metadata(&bin).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&bin, perms).unwrap();
    bin
}

fn resident(root: &Path, bin: &Path) -> Arc<Shared> {
    let rt = root.join("rt");
    std::fs::create_dir_all(&rt).unwrap();
    std::fs::write(rt.join("kamimusuhi.sqlite"), b"").unwrap();
    let config: Config = serde_json::from_value(json!({
        "node": {"id": "pi", "role": "continuity", "listen": "127.0.0.1:0"},
        "paths": {"root": root},
        "dialogue": {"runtime_binary": bin, "dir": rt, "timeout_secs": 10},
        // Dialogue turns are admitted one per subject process-wide, so each
        // test sends from its own peer (subject `sister-<from>`).
        "peers": [
            {"id": "mac", "role": "cognition", "url": "http://127.0.0.1:1"},
            {"id": "mac2", "role": "cognition", "url": "http://127.0.0.1:1"},
        ],
        "intercom": {"sisters": {"mac": {"label": "長女"}, "mac2": {"label": "次女"}}},
    }))
    .unwrap();
    config.validate().unwrap();
    let spool = Spool::new(root.join("spool"), "pi").unwrap();
    Arc::new(Shared::new(config, spool, 1, None))
}

fn envelope() -> Envelope {
    serde_json::from_value(json!({
        "id": "m-crash-1", "from": "mac", "to": "pi", "hop": 0,
        "kind": "message", "body": "落とさないでほしい伝言", "sent_at": 1,
    }))
    .unwrap()
}

fn state_file(root: &Path) -> PathBuf {
    root.join("current_state").join("intercom.json")
}

#[test]
fn an_envelope_in_flight_at_a_crash_is_not_lost_or_deduplicated_on_resend() {
    let live = tempfile::tempdir().unwrap();
    let marker = live.path().join("turn-started");
    // The scripted runtime proves it is inside the turn, then never answers.
    let bin = script(
        live.path(),
        &format!("touch '{}'\nsleep 10", marker.display()),
    );
    let shared = resident(live.path(), &bin);
    let (status, body) = shared.intercom.accept(&shared, envelope());
    assert_eq!((status, &body["accepted"]), (200, &json!(true)), "{body}");
    let accepted: Value =
        serde_json::from_slice(&std::fs::read(state_file(live.path())).unwrap()).unwrap();
    assert_eq!(accepted["inbound"].as_array().map(Vec::len), Some(1));

    intercom::spawn(&shared);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the turn never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    // The crash image: what is on disk while the turn is in flight.
    let image = std::fs::read(state_file(live.path())).unwrap();

    let restarted_root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(restarted_root.path().join("current_state")).unwrap();
    std::fs::write(state_file(restarted_root.path()), &image).unwrap();
    let idle = script(restarted_root.path(), "sleep 10");
    let restarted = resident(restarted_root.path(), &idle);

    // After restart the acknowledged envelope must still be processable:
    // either it is still queued, or the sender's resend is accepted.
    let on_disk: Value = serde_json::from_slice(&image).unwrap();
    let still_queued = on_disk["inbound"]
        .as_array()
        .is_some_and(|q| q.iter().any(|e| e["id"] == "m-crash-1"));
    let (_, resend) = restarted.intercom.accept(&restarted, envelope());
    assert!(
        still_queued || resend["accepted"] == json!(true),
        "acknowledged envelope lost: inbound={} seen={} resend={resend}",
        on_disk["inbound"],
        on_disk["seen"],
    );
}

/// Run `env` through a live resident whose runtime hangs inside the turn
/// and return the on-disk `intercom.json` captured mid-turn. The hung
/// runtime is then released (it exits without answering) so the live
/// resident stops holding the subject's turn slot; everything it does
/// afterwards lands in its own directory, not in the returned image.
fn crash_image_of(env: Envelope) -> (tempfile::TempDir, Vec<u8>) {
    let live = tempfile::tempdir().unwrap();
    let marker = live.path().join("turn-started");
    let release = live.path().join("release");
    let bin = script(
        live.path(),
        &format!(
            "touch '{}'\nwhile [ ! -e '{}' ]; do sleep 0.05; done\nexit 1",
            marker.display(),
            release.display()
        ),
    );
    let shared = resident(live.path(), &bin);
    let (status, body) = shared.intercom.accept(&shared, env);
    assert_eq!((status, &body["accepted"]), (200, &json!(true)), "{body}");
    intercom::spawn(&shared);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the turn never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let image = std::fs::read(state_file(live.path())).unwrap();
    std::fs::write(&release, b"").unwrap();
    (live, image)
}

fn journal(root: &Path, category: &str) -> Vec<Value> {
    let dir = root.join("spool").join(category).join("pi");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
        out.extend(text.lines().filter_map(|l| serde_json::from_str(l).ok()));
    }
    out
}

fn read_state(root: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(state_file(root)).unwrap()).unwrap()
}

fn replies_to(state: &Value, id: &str) -> usize {
    state["outbox"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["envelope"]["in_reply_to"] == json!(id))
        .count()
}

#[test]
fn an_envelope_in_flight_at_a_crash_is_answered_once_after_the_restart() {
    // An auto-reply conversation: the reply is forwarded verbatim, so the
    // outbox shows exactly how many replies the restart produced.
    let auto_env: Envelope = serde_json::from_value(json!({
        "id": "m-crash-2", "from": "mac2", "to": "pi", "hop": 0,
        "conversation": "cv-mac-crash", "auto_left": 3,
        "kind": "message", "body": "返事をひとつだけください", "sent_at": 1,
    }))
    .unwrap();
    let (_live, image) = crash_image_of(auto_env.clone());
    let on_disk: Value = serde_json::from_slice(&image).unwrap();
    assert_eq!(on_disk["in_flight"]["id"], json!("m-crash-2"), "{on_disk}");
    assert_eq!(on_disk["in_flight"]["starts"], json!(1));
    assert_eq!(replies_to(&on_disk, "m-crash-2"), 0);

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("current_state")).unwrap();
    std::fs::write(state_file(root.path()), &image).unwrap();
    let answering = script(
        root.path(),
        "printf '%s\\n' '{\"response\":\"ひとつだけ返すね\",\"turn_id\":\"t1\"}'",
    );
    let restarted = resident(root.path(), &answering);
    intercom::spawn(&restarted);
    let deadline = Instant::now() + Duration::from_secs(10);
    let settled = loop {
        let state = read_state(root.path());
        if state["inbound"].as_array().is_some_and(Vec::is_empty)
            && state.get("in_flight").is_none()
        {
            break state;
        }
        assert!(Instant::now() < deadline, "never settled: {state}");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(replies_to(&settled, "m-crash-2"), 1, "{settled}");
    let reply = settled["outbox"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["envelope"]["in_reply_to"] == json!("m-crash-2"))
        .unwrap();
    assert_eq!(reply["envelope"]["body"], json!("ひとつだけ返すね"));

    // The second start is journaled and told to the individual.
    let intercom_journal = journal(root.path(), "conversations/intercom");
    assert!(
        intercom_journal
            .iter()
            .any(|r| r["kind"] == json!("redelivered") && r["starts"] == json!(2)),
        "{intercom_journal:?}"
    );
    assert!(
        intercom_journal
            .iter()
            .any(|r| r["kind"] == json!("answered") && r["id"] == json!("m-crash-2")),
        "{intercom_journal:?}"
    );
    let dialogue_journal = journal(root.path(), "conversations/dialogue");
    let received: Vec<&Value> = dialogue_journal
        .iter()
        .filter(|r| r["kind"] == json!("received"))
        .collect();
    assert_eq!(received.len(), 1, "{dialogue_journal:?}");
    let message = received[0]["message"].as_str().unwrap();
    assert!(
        message.contains("もう一度届けています（2回目）"),
        "{message}"
    );
    assert!(message.contains("返事をひとつだけください"), "{message}");

    // The sender's resend is still deduplicated and queues nothing more.
    let (_, resend) = restarted.intercom.accept(&restarted, auto_env);
    assert_eq!(resend["dedup"], json!(true), "{resend}");
    let after = read_state(root.path());
    assert_eq!(replies_to(&after, "m-crash-2"), 1, "{after}");
    assert!(after["inbound"].as_array().is_some_and(Vec::is_empty));
}
