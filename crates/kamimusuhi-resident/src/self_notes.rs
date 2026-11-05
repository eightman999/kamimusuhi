//! `GET|POST /v1/kamimusuhi/self_notes`: the newest entries of the
//! individual's self-authored note file, for the operator dashboard.
//!
//! The resident owns `self_notes.jsonl` — it is a plain file beside the
//! canonical store, not canonical state — so unlike `/v1/kamimusuhi/memory`
//! this endpoint reads it directly instead of spawning the runtime. The
//! answer is bounded (a tail of at most 50 entries) and never cached: the
//! file is small and a fresh read is the honest one.

use serde_json::{Value, json};

use crate::state::Shared;

/// Returns (HTTP status, body).
pub fn self_notes(shared: &Shared, request: &Value) -> (u16, Value) {
    let Some(config) = &shared.config.dialogue else {
        return (
            404,
            json!({"error": "self notes are served by the node hosting the individual"}),
        );
    };
    if !config.dir.join("kamimusuhi.sqlite").is_file() {
        return (
            503,
            json!({"error": "individual is not initialized on this node"}),
        );
    }
    let limit = request["limit"].as_u64().unwrap_or(10).clamp(1, 50) as usize;
    let file = kamimusuhi_runtime::self_notes::path(&config.dir);
    let notes = kamimusuhi_runtime::self_notes::latest(&config.dir, limit);
    (
        200,
        json!({
            "self_notes": notes,
            "count": notes.len(),
            "file_present": file.is_file(),
        }),
    )
}
