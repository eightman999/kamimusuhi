//! `self_notes.jsonl`: the individual's bounded, self-authored note file.
//!
//! The file lives inside the runtime directory, next to — and deliberately
//! not inside — the canonical store. The resident's `self_note` tool appends
//! through [`append`]; workspace assembly reads the newest entries through
//! [`latest`]. Losing the file loses only scratch state: a note is a working
//! hypothesis, and the durable lanes (canonical evidence, activated memory,
//! the C0 self model) hold everything that must survive.
//!
//! The format is one JSON [`SelfNote`] per line, oldest first. Rotation
//! keeps the newest [`MAX_NOTES_KEPT`] lines; a malformed line is dropped on
//! read and by the next rotation — it is never repaired into something it
//! was not.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kamimusuhi_core::self_note::{MAX_NOTES_KEPT, SelfNote, SelfNoteKind};
use kamimusuhi_core::time::UtcTimestamp;

use crate::RuntimeError;

pub const FILE_NAME: &str = "self_notes.jsonl";

/// Reads within one process serialize appends; concurrent turns all reach
/// the file through the resident, so this mutex is the write boundary.
static APPEND_LOCK: Mutex<()> = Mutex::new(());

/// Where the note file lives inside a runtime directory.
pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Every parseable note in the file, oldest first. A missing file is an
/// empty lane, not an error; a malformed line is skipped, never fatal.
pub fn load(dir: &Path) -> Result<Vec<SelfNote>, RuntimeError> {
    let file = path(dir);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(RuntimeError::DirectoryIo {
                path: file.display().to_string(),
                message: e.to_string(),
            });
        }
    };
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<SelfNote>(line).ok())
        .collect())
}

/// The newest `max` notes, oldest first — the order workspace assembly
/// stages them. Fail-soft like every dialogue-context read: an unreadable
/// or corrupt file means no notes this turn, never a failed turn.
pub fn latest(dir: &Path, max: usize) -> Vec<SelfNote> {
    if max == 0 {
        return Vec::new();
    }
    let notes = load(dir).unwrap_or_default();
    let start = notes.len().saturating_sub(max);
    notes.into_iter().skip(start).collect()
}

/// Append one note and rotate the file to its newest [`MAX_NOTES_KEPT`]
/// entries. The note's `seq` continues from the highest surviving sequence
/// so labels stay monotonic across rotations.
///
/// Bounds are enforced by [`SelfNote::checked`]: a refusal reaches the
/// caller as `RuntimeError::Usage`, and nothing is written.
pub fn append(
    dir: &Path,
    kind: SelfNoteKind,
    text: &str,
    evidence_refs: Vec<String>,
    at: UtcTimestamp,
) -> Result<SelfNote, RuntimeError> {
    let _guard = APPEND_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut notes = load(dir)?;
    let seq = notes
        .iter()
        .map(|note| note.seq)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let note = SelfNote::checked(seq, kind, text.to_owned(), evidence_refs, at)
        .map_err(|e| RuntimeError::Usage(e.to_string()))?;
    notes.push(note.clone());
    if notes.len() > MAX_NOTES_KEPT {
        notes.drain(..notes.len() - MAX_NOTES_KEPT);
    }
    write_all(dir, &notes)?;
    Ok(note)
}

/// Rewrite the file atomically (sibling temp + rename), the same shape the
/// resident's `atomic_write` uses — a crash leaves the previous file or the
/// new one, never a torn line.
fn write_all(dir: &Path, notes: &[SelfNote]) -> Result<(), RuntimeError> {
    let file = path(dir);
    let tmp = dir.join(format!("{FILE_NAME}.tmp-{}", std::process::id()));
    let io = |context: &str, e: std::io::Error| RuntimeError::DirectoryIo {
        path: format!("{} ({context})", file.display()),
        message: e.to_string(),
    };
    {
        let mut out = std::fs::File::create(&tmp).map_err(|e| io("create", e))?;
        for note in notes {
            let line = serde_json::to_string(note)
                .map_err(|e| RuntimeError::Usage(format!("could not encode self note: {e}")))?;
            writeln!(out, "{line}").map_err(|e| io("write", e))?;
        }
        out.sync_all().map_err(|e| io("sync", e))?;
    }
    std::fs::rename(&tmp, &file).map_err(|e| io("rename", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kamimusuhi_core::self_note::MAX_EVIDENCE_REFS;

    fn at() -> UtcTimestamp {
        UtcTimestamp::from_unix_millis(1_000)
    }

    #[test]
    fn a_missing_file_is_an_empty_lane() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(load(dir.path()).expect("load").is_empty());
        assert!(latest(dir.path(), 5).is_empty());
    }

    #[test]
    fn append_assigns_sequences_and_latest_takes_the_newest() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..8 {
            let note = append(
                dir.path(),
                SelfNoteKind::Observation,
                &format!("note {i}"),
                vec![],
                at(),
            )
            .expect("append");
            assert_eq!(note.seq, i as u64 + 1);
        }
        let newest = latest(dir.path(), 3);
        assert_eq!(
            newest.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
            vec!["note 5", "note 6", "note 7"]
        );
        // More than the file holds comes back whole; zero disables the lane.
        assert_eq!(latest(dir.path(), 50).len(), 8);
        assert!(latest(dir.path(), 0).is_empty());
    }

    #[test]
    fn rotation_keeps_the_newest_and_continues_the_sequence() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..(MAX_NOTES_KEPT + 10) {
            append(
                dir.path(),
                SelfNoteKind::Tendency,
                &format!("n{i}"),
                vec![],
                at(),
            )
            .expect("append");
        }
        let kept = load(dir.path()).expect("load");
        assert_eq!(kept.len(), MAX_NOTES_KEPT);
        assert_eq!(kept.first().expect("first").text, "n10");
        assert_eq!(kept.last().expect("last").seq, MAX_NOTES_KEPT as u64 + 10);
        // The next append continues the sequence rather than restarting it.
        let next = append(dir.path(), SelfNoteKind::Lesson, "fresh", vec![], at()).unwrap();
        assert_eq!(next.seq, MAX_NOTES_KEPT as u64 + 11);
    }

    #[test]
    fn malformed_lines_are_dropped_not_fatally_parsed() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            path(dir.path()),
            "{\"seq\":1,\"kind\":\"lesson\",\"text\":\"ok\",\"created_at\":0}\n\
             not json\n\
             {\"seq\":2,\"kind\":\"unknown\",\"text\":\"bad kind\",\"created_at\":0}\n\
             {\"seq\":3,\"kind\":\"preference\",\"text\":\"also ok\",\"created_at\":0}\n",
        )
        .expect("write");
        let notes = load(dir.path()).expect("load");
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].seq, 1);
        assert_eq!(notes[1].seq, 3);
    }

    #[test]
    fn bounds_are_refused_without_writing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            append(
                dir.path(),
                SelfNoteKind::Lesson,
                &"x".repeat(1025),
                vec![],
                at()
            ),
            Err(RuntimeError::Usage(_))
        ));
        assert!(matches!(
            append(
                dir.path(),
                SelfNoteKind::Lesson,
                "t",
                (0..=MAX_EVIDENCE_REFS).map(|i| i.to_string()).collect(),
                at(),
            ),
            Err(RuntimeError::Usage(_))
        ));
        assert!(!path(dir.path()).exists());
    }
}
