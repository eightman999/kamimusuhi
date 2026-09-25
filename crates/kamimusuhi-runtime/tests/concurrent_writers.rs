//! Several processes running turns against one runtime directory.
//!
//! The resident runs each dialogue turn as its own `kamimusuhi-runtime talk`
//! process, and more than one may be in flight. Each claims its own writer
//! epoch; a turn whose epoch was fenced by a later claim must re-claim
//! rather than have its drafts rejected as stale.

use kamimusuhi_runtime::config::ReflectorBackend;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};
use kamimusuhi_testkit::fake_persona::FIXTURE_USER_SUBJECT;

fn init(dir: &std::path::Path) {
    let mut runtime = Runtime::init(
        dir,
        RuntimeOptions::deterministic(10),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let mut config = runtime.config().clone();
    config.c0.reflector.backend = ReflectorBackend::Fake;
    runtime.save_config(config).unwrap();
    runtime.stopping();
}

fn open(dir: &std::path::Path, seed: u64) -> Runtime {
    Runtime::open(dir, RuntimeOptions::deterministic(seed)).unwrap()
}

fn activated(session: &mut DialogueSession, runtime: &mut Runtime, text: &str) -> usize {
    let reply = session.turn(runtime, text, |_| Ok(())).unwrap();
    reply.c0.as_ref().unwrap().drafts_activated
}

#[test]
fn a_fenced_session_reclaims_instead_of_losing_its_drafts() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    let mut a = open(dir.path(), 10_000);
    let mut b = open(dir.path(), 20_000);
    let mut session_a =
        DialogueSession::start(&mut a, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
    let mut session_b =
        DialogueSession::start(&mut b, FIXTURE_USER_SUBJECT, Default::default()).unwrap();

    // A claims an epoch, then B claims a later one and fences A out.
    assert_eq!(
        activated(&mut session_a, &mut a, "私はほうじ茶が好き。覚えておいて"),
        1
    );
    assert_eq!(
        activated(&mut session_b, &mut b, "私は緑茶が好き。覚えておいて"),
        1
    );
    // A's cached identity is now stale; its next draft must still land.
    assert_eq!(
        activated(&mut session_a, &mut a, "私は麦茶が好き。覚えておいて"),
        1
    );
    a.stopping();
    b.stopping();
}

#[test]
fn concurrent_turns_in_separate_runtimes_all_activate() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    let handles: Vec<_> = [10_000_u64, 20_000, 30_000]
        .into_iter()
        .map(|seed| {
            let dir = dir.path().to_owned();
            std::thread::spawn(move || {
                let mut runtime = open(&dir, seed);
                let mut session =
                    DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default())
                        .unwrap();
                let mut total = 0;
                for drink in ["ほうじ茶", "緑茶"] {
                    let text = format!("私は{drink}{seed}が好き。覚えておいて");
                    total += activated(&mut session, &mut runtime, &text);
                }
                runtime.stopping();
                total
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), 2);
    }
}
