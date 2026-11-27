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
        "peers": [{"id": "mac", "role": "cognition", "url": "http://127.0.0.1:1"}],
        "intercom": {"sisters": {"mac": {"label": "長女"}}},
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
#[ignore = "defect: intercom inbound worker persists the pop before the turn runs; a crash mid-turn loses an acknowledged envelope and its resend is deduplicated (see docs/verification/write-fault-shapes-2026-11-27.md)"]
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
