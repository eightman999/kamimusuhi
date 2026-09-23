//! Task plane orchestration: delegating work to external agent harnesses
//! without holding up the conversation.
//!
//! `task_delegate` validates the request, picks an executor and model,
//! records a `delegated` task on the board and returns — the chat turn
//! never waits for the harness. A worker thread then runs the task
//! (bounded by per-node and per-executor concurrency), streams progress to
//! the board, and stores the result as an *external task result* evidence
//! record under `current_state/task-results/` (and `logs/task-results/`
//! for the NAS). The result is what an outside agent reported; the
//! individual reads it through `task_status`, where it becomes ordinary
//! resource-result evidence of that turn. Nothing here writes canonical
//! identity or memory.
//!
//! Executors are configuration (`task_plane.executors` and
//! `task_plane.executors_file`). The file is re-read when it changes, so
//! a harness can be added or removed while tasks are running; a running
//! task keeps the executor it started with.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use kamimusuhi_runtime::agent_exec::history::{
    Battery, EvalCase, Feedback, HistoryRecord, Verdict,
};
use kamimusuhi_runtime::agent_exec::{
    self as ax, AgentBilling, ExecutorConfig, ExecutorRegistry, ExecutorSpec, ModelCatalog, RunEnd,
    TaskEvent, TaskExecutor, TaskKind, TaskOutcome, TaskPermissions, TaskRequest,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::{PathsConfig, RoutingMode, TaskPlaneConfig};
use crate::spool::Spool;
use crate::state::Shared;
use crate::task_ledger::{CostLedger, HistoryStore, Reservation, TOTAL};
use crate::task_worktree::{self, Worktree};
use crate::tasks::{NewTask, TaskBoard, TaskStatus};
use crate::util::{atomic_write, iso8601, run_with_timeout, unix_now};

/// Tools the individual gets when this node (or a peer) has a task plane.
pub const TOOL_NAMES: [&str; 6] = [
    "task_delegate",
    "task_status",
    "task_cancel",
    "task_continue",
    "agent_models",
    "agent_health",
];

const OBJECTIVE_CHARS: usize = 8_000;
const CONTEXT_TOTAL_CHARS: usize = 16_000;
const LIST_ITEMS: usize = 10;
const ITEM_CHARS: usize = 500;
/// Result text returned to the individual per `task_status` call.
const STATUS_SUMMARY_CHARS: usize = 4_000;
/// Result files kept locally (the NAS keeps them all via the spool).
const KEEP_RESULTS: usize = 500;
/// Minimum spacing of progress notes on the board.
const NOTE_EVERY: Duration = Duration::from_secs(15);

/// `executors_file` contents. Other top-level keys (`_comment`) are
/// ignored; each executor entry is still strict.
#[derive(Deserialize)]
struct ExecutorsFile {
    #[serde(default)]
    executors: Vec<ExecutorConfig>,
}

pub fn parse_executors_file(bytes: &[u8]) -> Result<Vec<ExecutorConfig>, String> {
    serde_json::from_slice::<ExecutorsFile>(bytes)
        .map(|f| f.executors)
        .map_err(|e| e.to_string())
}

struct RegistryState {
    registry: ExecutorRegistry,
    /// (mtime, length) of `executors_file` when last read.
    file_stamp: Option<(SystemTime, u64)>,
    error: Option<String>,
    loaded_at: u64,
}

#[derive(Default)]
struct Slots {
    total: usize,
    per_executor: HashMap<String, usize>,
}

pub struct TaskPlane {
    config: TaskPlaneConfig,
    registry: RwLock<RegistryState>,
    catalog: Mutex<ModelCatalog>,
    catalog_path: PathBuf,
    results_dir: PathBuf,
    /// Cancel flags of queued and running tasks.
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
    slots: Mutex<Slots>,
    slot_freed: Condvar,
    force_refresh: AtomicBool,
    ledger: CostLedger,
    history: HistoryStore,
    /// `<root>/worktrees`: one git worktree per write task.
    worktrees_dir: PathBuf,
}

fn file_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn load_specs(config: &TaskPlaneConfig) -> Result<Vec<ExecutorSpec>, String> {
    let mut all = config.executors.clone();
    if let Some(path) = &config.executors_file {
        match std::fs::read(path) {
            Ok(bytes) => {
                all.extend(
                    parse_executors_file(&bytes).map_err(|e| format!("{}: {e}", path.display()))?,
                );
            }
            // A missing file is an empty list: executors can be added by
            // creating it later.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    ax::resolve_all(&all)
}

fn clip(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

fn strings(value: &Value, items: usize, chars: usize) -> Vec<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => vec![clip(s.trim(), chars)],
        Value::Array(list) => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .take(items)
            .map(|s| clip(s, chars))
            .collect(),
        _ => Vec::new(),
    }
}

/// Working-tree state used to prove a read-only task stayed read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeState {
    entries: HashSet<String>,
    diff_hash: u64,
}

fn tree_state(workspace: &Path) -> Option<TreeState> {
    use std::hash::{Hash, Hasher};
    let ws = workspace.to_string_lossy();
    let timeout = Duration::from_secs(20);
    let status = run_with_timeout(
        "git",
        &[
            "-C",
            &ws,
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ],
        timeout,
    )
    .filter(|o| o.success)?;
    let diff = run_with_timeout(
        "git",
        &["-C", &ws, "diff", "HEAD", "--no-ext-diff"],
        timeout,
    )
    .map(|o| o.stdout)
    .unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    diff.hash(&mut hasher);
    Some(TreeState {
        entries: status.stdout.lines().map(str::to_owned).collect(),
        diff_hash: hasher.finish(),
    })
}

/// Files whose state changed between two snapshots, and whether anything
/// changed at all (an already-modified file edited again shows only in
/// the diff).
fn tree_changes(before: &TreeState, after: &TreeState) -> (Vec<String>, bool) {
    let mut files: Vec<String> = after
        .entries
        .symmetric_difference(&before.entries)
        .map(|line| line.get(3..).unwrap_or(line).to_owned())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    files.sort();
    let changed = !files.is_empty() || before.diff_hash != after.diff_hash;
    (files, changed)
}

impl TaskPlane {
    pub fn new(config: TaskPlaneConfig, paths: &PathsConfig) -> Self {
        let (registry, error) = match load_specs(&config) {
            Ok(specs) => (ExecutorRegistry::from_specs(specs), None),
            Err(e) => (ExecutorRegistry::default(), Some(e)),
        };
        let file_stamp = config.executors_file.as_deref().and_then(file_stamp);
        let catalog_path = paths.cache().join("agent-model-catalog.json");
        let mut catalog = ModelCatalog::load(&catalog_path);
        catalog.retain(&registry.names());
        Self {
            registry: RwLock::new(RegistryState {
                registry,
                file_stamp,
                error,
                loaded_at: unix_now(),
            }),
            catalog: Mutex::new(catalog),
            catalog_path,
            results_dir: paths.current_state().join("task-results"),
            running: Mutex::new(HashMap::new()),
            slots: Mutex::new(Slots::default()),
            slot_freed: Condvar::new(),
            force_refresh: AtomicBool::new(true),
            ledger: CostLedger::load(paths.current_state().join("task-cost-ledger.json")),
            history: HistoryStore::new(paths.current_state()),
            worktrees_dir: paths.root.join("worktrees"),
            config,
        }
    }

    /// Re-read `executors_file` if it changed. A bad file keeps the
    /// previous executors and reports the error.
    pub fn reload_if_changed(&self) {
        let Some(path) = &self.config.executors_file else {
            return;
        };
        let stamp = file_stamp(path);
        if self
            .registry
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .file_stamp
            == stamp
        {
            return;
        }
        let mut state = self.registry.write().unwrap_or_else(|p| p.into_inner());
        if state.file_stamp == stamp {
            return;
        }
        state.file_stamp = stamp;
        match load_specs(&self.config) {
            Ok(specs) => {
                state.registry = ExecutorRegistry::from_specs(specs);
                state.error = None;
                state.loaded_at = unix_now();
                self.catalog
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .retain(&state.registry.names());
                self.force_refresh.store(true, Ordering::Relaxed);
            }
            Err(e) => state.error = Some(e),
        }
    }

    pub fn registry(&self) -> ExecutorRegistry {
        self.reload_if_changed();
        self.registry
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .registry
            .clone()
    }

    fn catalog_snapshot(&self) -> ModelCatalog {
        self.catalog
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn allows_write(&self, name: &str) -> bool {
        self.config
            .workspaces
            .iter()
            .any(|w| w.name == name && w.allow_write)
    }

    fn workspace(&self, name: Option<&str>) -> Result<(String, PathBuf), String> {
        let spaces = &self.config.workspaces;
        let ws = match name.filter(|n| !n.is_empty()) {
            Some(n) => spaces.iter().find(|w| w.name == n).ok_or_else(|| {
                format!(
                    "workspace {n} は許可されていない（{}）",
                    spaces
                        .iter()
                        .map(|w| w.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?,
            None if spaces.len() == 1 => &spaces[0],
            None => {
                return Err(format!(
                    "workspace を指定する（{}）",
                    spaces
                        .iter()
                        .map(|w| w.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        };
        let path = ws
            .path
            .canonicalize()
            .map_err(|e| format!("workspace {}: {e}", ws.name))?;
        if !path.is_dir() {
            return Err(format!("workspace {} is not a directory", ws.name));
        }
        Ok((ws.name.clone(), path))
    }

    /// Discover models for executors whose catalog is stale (or all, when
    /// a refresh was requested). Runs on the task-plane thread.
    pub fn refresh_catalog(&self) {
        let force = self.force_refresh.swap(false, Ordering::Relaxed);
        let now = unix_now();
        for executor in self.registry().all() {
            let spec = executor.spec();
            if !spec.enabled || !executor.health().ok {
                continue;
            }
            let stale = self
                .catalog
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .executors
                .get(&spec.name)
                .is_none_or(|e| e.is_stale(spec.catalog_ttl_secs, now));
            if !(force || stale) {
                continue;
            }
            let mut entry = executor.discover(unix_now());
            let mut catalog = self.catalog.lock().unwrap_or_else(|p| p.into_inner());
            // A failed refresh keeps the last good model list.
            if entry.error.is_some()
                && let Some(previous) = catalog.executors.get(&spec.name)
            {
                entry.models.clone_from(&previous.models);
                entry.discovered_at = previous.discovered_at;
            }
            catalog.executors.insert(spec.name.clone(), entry);
            let _ = catalog.save(&self.catalog_path);
        }
    }

    pub fn request_refresh(&self) {
        self.force_refresh.store(true, Ordering::Relaxed);
    }

    fn acquire(&self, executor: &ExecutorSpec, cancel: &AtomicBool) -> bool {
        let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            let mine = slots.per_executor.get(&executor.name).copied().unwrap_or(0);
            if slots.total < self.config.max_concurrent && mine < executor.max_concurrent {
                slots.total += 1;
                *slots.per_executor.entry(executor.name.clone()).or_default() += 1;
                return true;
            }
            slots = self
                .slot_freed
                .wait_timeout(slots, Duration::from_secs(1))
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    fn release(&self, executor: &str) {
        let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        slots.total = slots.total.saturating_sub(1);
        if let Some(n) = slots.per_executor.get_mut(executor) {
            *n = n.saturating_sub(1);
        }
        drop(slots);
        self.slot_freed.notify_all();
    }

    fn result_path(&self, task_id: &str) -> PathBuf {
        self.results_dir.join(format!("{task_id}.json"))
    }

    fn load_result(&self, task_id: &str) -> Option<Value> {
        std::fs::read(self.result_path(task_id))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    }

    fn store_result(&self, spool: &Spool, record: &Value) {
        let id = record["task_id"].as_str().unwrap_or("unknown");
        let _ = std::fs::create_dir_all(&self.results_dir);
        let _ = atomic_write(
            &self.result_path(id),
            &serde_json::to_vec_pretty(record).unwrap_or_default(),
        );
        let _ = spool.append("logs/task-results", record.clone());
        // Keep the newest results locally.
        if let Ok(dir) = std::fs::read_dir(&self.results_dir) {
            let mut files: Vec<(SystemTime, PathBuf)> = dir
                .flatten()
                .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                .collect();
            if files.len() > KEEP_RESULTS {
                files.sort();
                for (_, path) in &files[..files.len() - KEEP_RESULTS] {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    /// Board-independent view for `/status`.
    pub fn status(&self) -> Value {
        let registry = self.registry();
        let (state_error, loaded_at) = {
            let state = self.registry.read().unwrap_or_else(|p| p.into_inner());
            (state.error.clone(), state.loaded_at)
        };
        let catalog = self.catalog_snapshot();
        let slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        let executors: Vec<Value> = registry
            .all()
            .iter()
            .map(|e| {
                let health = e.health();
                let entry = catalog.executors.get(&e.spec().name);
                json!({
                    "name": health.executor,
                    "adapter": health.adapter,
                    "enabled": health.enabled,
                    "ok": health.ok,
                    "detail": health.detail,
                    "binary": health.binary,
                    "running": slots.per_executor.get(&e.spec().name).copied().unwrap_or(0),
                    "max_concurrent": e.spec().max_concurrent,
                    "default_model": e.spec().default_model,
                    "default_billing": e.spec().default_billing.as_str(),
                    "models": entry.map_or(0, |c| c.models.len()),
                    "catalog_checked": entry.filter(|c| c.checked_at > 0).map(|c| iso8601(c.checked_at)),
                    "catalog_error": entry.and_then(|c| c.error.clone()),
                    "harness_version": entry.and_then(|c| c.harness_version.clone()),
                    "protocol": e.spec().protocol.as_str(),
                    "can_resume": e.can_resume(),
                    "supervisor": e.supervisor(),
                    "quota": (!e.spec().quota.is_empty()).then(|| e.spec().quota.clone()),
                })
            })
            .collect();
        json!({
            "executors": executors,
            "config_error": state_error,
            "executors_loaded": iso8601(loaded_at),
            "running": slots.total,
            "queued_or_running": self.running.lock().unwrap_or_else(|p| p.into_inner()).len(),
            "max_concurrent": self.config.max_concurrent,
            "routing_mode": self.config.routing_mode.as_str(),
            "quota": self.config.quota,
            "workspaces": self.config.workspaces.iter().map(|w| json!({
                "name": w.name, "description": w.description})).collect::<Vec<_>>(),
        })
    }
}

/// Catalog upkeep: re-read `executors_file` and refresh stale model lists.
pub fn spawn(shared: &Arc<Shared>) {
    let Some(plane) = shared.task_plane.clone() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("task-plane".to_owned())
        .spawn(move || {
            loop {
                plane.reload_if_changed();
                // Start/restart attached servers, stop idle ACP servers.
                for executor in plane.registry().all() {
                    executor.maintain();
                }
                plane.refresh_catalog();
                let started = Instant::now();
                let interval = Duration::from_secs(plane.config.check_interval_secs);
                while started.elapsed() < interval && !plane.force_refresh.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        });
}

fn plane(shared: &Shared) -> Result<&Arc<TaskPlane>, (u16, Value)> {
    shared
        .task_plane
        .as_ref()
        .ok_or((404, json!({"error": "this node has no task plane"})))
}

/// Everything a worker needs, detached from `Shared`.
struct Job {
    plane: Arc<TaskPlane>,
    board: Arc<TaskBoard>,
    spool: Arc<Spool>,
    node: String,
    executor: Arc<dyn TaskExecutor>,
    selection: ax::Selection,
    workspace_name: String,
    request: TaskRequest,
    cancel: Arc<AtomicBool>,
    by: String,
    reservation: Reservation,
    eval: Option<(String, EvalCase)>,
    shadow_choice: Option<String>,
    /// For write tasks: the worktree to continue in (else a new one).
    reuse_worktree: Option<Worktree>,
}

/// A task about to be admitted: what to run, where, on what, and why.
struct Start {
    title: String,
    request: TaskRequest,
    workspace_name: String,
    selection: ax::Selection,
    depends_on: Vec<String>,
    by: String,
    continues: Option<String>,
    eval: Option<(String, EvalCase)>,
    shadow_choice: Option<String>,
    reuse_worktree: Option<Worktree>,
}

fn parse_kind(value: &Value) -> Result<TaskKind, (u16, Value)> {
    match value.as_str() {
        None | Some("") => Ok(TaskKind::General),
        Some(k) => TaskKind::parse(k).ok_or((400, json!({"error": format!("unknown kind {k}")}))),
    }
}

fn candidates(registry: &ExecutorRegistry) -> Vec<(Arc<dyn TaskExecutor>, bool)> {
    registry
        .all()
        .iter()
        .map(|e| (Arc::clone(e), e.health().ok))
        .collect()
}

/// The single admission path for delegation, continuation and
/// evaluation: budget and quota checks, the board task, the worker.
fn start_task(
    shared: &Shared,
    plane: &Arc<TaskPlane>,
    start: Start,
) -> Result<Value, (u16, Value)> {
    let Some(executor) = plane.registry().get(&start.selection.executor) else {
        return Err((
            400,
            json!({"error": format!("executor {} は設定されていない", start.selection.executor)}),
        ));
    };
    let spec = executor.spec();
    let total_quota = plane.config.quota.clone().unwrap_or_default();
    if start.selection.billing == AgentBilling::Metered
        && !(spec.quota.has_usd_budget() || total_quota.has_usd_budget())
    {
        return Err((
            400,
            json!({"error": format!(
                "{} は従量課金。executor か task_plane の quota に費用上限（max_usd_per_day/month）がない限り使わない",
                start.selection.executor)}),
        ));
    }
    let reservation = plane
        .ledger
        .reserve(&spec.name, &spec.quota, &total_quota, unix_now())
        .map_err(|e| (429, json!({"error": e})))?;
    let selection = start.selection;
    let model_ref = selection
        .model
        .as_deref()
        .map(|m| format!("{}:{m}", selection.executor));
    let task_id = shared.tasks.create(
        &shared.spool,
        NewTask {
            title: &start.title,
            kind: "delegated",
            node: &shared.config.node.id,
            owner: &start.by,
            status: TaskStatus::Waiting,
            depends_on: start.depends_on,
            detail: json!({
                "plane": "task",
                "task_kind": start.request.kind.as_str(),
                "lane": start.request.kind.lane(),
                "executor": selection.executor,
                "protocol": spec.protocol.as_str(),
                "model": selection.model,
                "model_ref": model_ref,
                "billing": selection.billing.as_str(),
                "billing_source": selection.billing_source,
                "routing": selection.reason,
                "shadow_routing": start.shadow_choice,
                "warnings": selection.warnings,
                "workspace": start.workspace_name,
                "permissions": start.request.permissions.as_str(),
                "continues": start.continues,
                "eval": start.eval.as_ref().map(|(run, case)| json!({"run_id": run, "case": case.id})),
                "worktree": start.reuse_worktree.as_ref().map(|w| w.path.display().to_string()),
                "branch": start.reuse_worktree.as_ref().map(|w| w.branch.clone()),
                "base_commit": start.reuse_worktree.as_ref().map(|w| w.base.clone()),
            }),
        },
    );
    let mut request = start.request;
    request.task_id.clone_from(&task_id);
    let cancel = Arc::new(AtomicBool::new(false));
    plane
        .running
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(task_id.clone(), Arc::clone(&cancel));
    let reply = json!({
        "task_id": task_id,
        "status": "waiting",
        "executor": selection.executor,
        "model": selection.model.clone().unwrap_or_else(|| "(harness default)".to_owned()),
        "billing": selection.billing.as_str(),
        "routing": selection.reason,
        "warnings": selection.warnings,
        "message": "タスクに回した。結果は task_status で確認できる（完了を待たずに会話を続けてよい）",
    });
    let job = Job {
        plane: Arc::clone(plane),
        board: Arc::clone(&shared.tasks),
        spool: Arc::clone(&shared.spool),
        node: shared.config.node.id.clone(),
        executor,
        selection,
        workspace_name: start.workspace_name,
        request,
        cancel,
        by: start.by,
        reservation: reservation.clone(),
        eval: start.eval,
        shadow_choice: start.shadow_choice,
        reuse_worktree: start.reuse_worktree,
    };
    let spawned = std::thread::Builder::new()
        .name(format!("task-{task_id}"))
        .spawn(move || run_job(job));
    if let Err(e) = spawned {
        plane
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&task_id);
        plane.ledger.settle(&reservation, None, None, true);
        shared.tasks.update(
            &shared.spool,
            &task_id,
            Some(TaskStatus::Failed),
            Some(("system", &format!("worker を起動できない: {e}"))),
            None,
        );
        return Err((
            500,
            json!({"error": format!("could not start worker: {e}")}),
        ));
    }
    Ok(reply)
}

/// `task_delegate`: create the task and return at once.
pub fn delegate(shared: &Shared, args: &Value, by: &str) -> (u16, Value) {
    let plane = match plane(shared) {
        Ok(p) => Arc::clone(p),
        Err(e) => return e,
    };
    let objective = args["objective"].as_str().unwrap_or("").trim();
    if objective.is_empty() || objective.chars().count() > OBJECTIVE_CHARS {
        return (400, json!({"error": "objective must be 1..8000 chars"}));
    }
    let kind = match parse_kind(&args["kind"]) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let permissions = match args["permissions"].as_str() {
        None | Some("") => TaskPermissions::ReadOnly,
        Some(p) => match TaskPermissions::parse(p) {
            Some(p) => p,
            None => return (400, json!({"error": format!("unknown permissions {p}")})),
        },
    };
    let (workspace_name, workspace) = match plane.workspace(args["workspace"].as_str()) {
        Ok(w) => w,
        Err(e) => return (400, json!({"error": e})),
    };
    if permissions != TaskPermissions::ReadOnly {
        if !plane.allows_write(&workspace_name) {
            return (
                400,
                json!({"error": format!("workspace {workspace_name} は書き込みタスクを許可していない（allow_write）")}),
            );
        }
        if permissions == TaskPermissions::AutonomousWorkspace && by != "operator" {
            return (
                403,
                json!({"error": "autonomous_workspace は操作者だけが指定できる"}),
            );
        }
        if !task_worktree::is_repo(&workspace) {
            return (
                400,
                json!({"error": "書き込みタスクの workspace は git リポジトリの最上位である必要がある"}),
            );
        }
    }
    let registry = plane.registry();
    let pool = candidates(&registry);
    let cands: Vec<ax::Candidate<'_>> = pool
        .iter()
        .map(|(e, ok)| ax::Candidate {
            spec: e.spec(),
            healthy: *ok,
        })
        .collect();
    let catalog = plane.catalog_snapshot();
    let executor_arg = args["executor"]
        .as_str()
        .filter(|e| !e.is_empty() && *e != "auto");
    let model_arg = args["model"].as_str().filter(|m| !m.trim().is_empty());
    let explicit = executor_arg.is_some() || model_arg.is_some();
    let static_pick = ax::select(kind, executor_arg, model_arg, &cands, &catalog);
    let (selection, shadow_choice) = if explicit {
        (static_pick, None)
    } else {
        let history = match plane.config.routing_mode {
            RoutingMode::Static => None,
            _ => {
                let (records, feedback) = plane.history.load();
                ax::policy::select_by_history(
                    kind,
                    &cands,
                    &catalog,
                    &ax::history::stats(&records, &feedback),
                    plane.config.history_min_samples,
                )
            }
        };
        let label = |s: &ax::Selection| {
            format!(
                "{}:{}",
                s.executor,
                s.model.as_deref().unwrap_or("(default)")
            )
        };
        match (plane.config.routing_mode, history) {
            (RoutingMode::History, Some(h)) => {
                let other = static_pick.as_ref().ok().map(label);
                (Ok(h), other)
            }
            (RoutingMode::Shadow, Some(h)) => (static_pick, Some(label(&h))),
            (_, _) => (static_pick, None),
        }
    };
    let selection = match selection {
        Ok(s) => s,
        Err(e) => return (400, json!({"error": e})),
    };
    let mut context = strings(&args["context"], LIST_ITEMS, ax::policy::CONTEXT_CHARS);
    let mut budget = CONTEXT_TOTAL_CHARS;
    context.retain_mut(|c| {
        let n = c.chars().count();
        if n > budget {
            return false;
        }
        budget -= n;
        true
    });
    let depends_on: Vec<String> = if by == "operator" {
        Vec::new()
    } else {
        shared
            .current_turn
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .into_iter()
            .collect()
    };
    let request = TaskRequest {
        task_id: String::new(),
        kind,
        objective: objective.to_owned(),
        success_criteria: strings(&args["success_criteria"], LIST_ITEMS, ITEM_CHARS),
        workspace,
        context,
        constraints: strings(&args["constraints"], LIST_ITEMS, ITEM_CHARS),
        permissions,
        model: selection.model.clone(),
        resume_session: None,
    };
    let start = Start {
        title: format!("委譲: {objective}"),
        request,
        workspace_name,
        selection,
        depends_on,
        by: by.to_owned(),
        continues: None,
        eval: None,
        shadow_choice,
        reuse_worktree: None,
    };
    match start_task(shared, &plane, start) {
        Ok(reply) => (200, reply),
        Err(e) => e,
    }
}

/// `task_continue`: a follow-up instruction in the session of a finished
/// task, on the same harness, model, workspace and permissions.
pub fn continue_task(shared: &Shared, args: &Value, by: &str) -> (u16, Value) {
    let plane = match plane(shared) {
        Ok(p) => Arc::clone(p),
        Err(e) => return e,
    };
    let original = match delegated(shared, args) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let instruction = args["instruction"].as_str().unwrap_or("").trim();
    if instruction.is_empty() || instruction.chars().count() > OBJECTIVE_CHARS {
        return (400, json!({"error": "instruction must be 1..8000 chars"}));
    }
    if !matches!(
        original.status,
        TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled
    ) {
        return (
            409,
            json!({"error": format!("{} はまだ終わっていない", original.id)}),
        );
    }
    let Some(result) = plane.load_result(&original.id) else {
        return (409, json!({"error": "結果が見つからない"}));
    };
    let Some(session) = result["session_id"].as_str().filter(|s| !s.is_empty()) else {
        return (
            409,
            json!({"error": "外部セッション id がないため続けられない（Devin は protocol: acp が必要）"}),
        );
    };
    let detail = &original.detail;
    let executor_name = detail["executor"].as_str().unwrap_or("");
    let Some(executor) = plane.registry().get(executor_name) else {
        return (
            400,
            json!({"error": format!("executor {executor_name} は設定から外れている")}),
        );
    };
    if !executor.can_resume() {
        return (
            400,
            json!({"error": format!("{executor_name} はセッションを継続できない")}),
        );
    }
    let (workspace_name, workspace) = match plane.workspace(detail["workspace"].as_str()) {
        Ok(w) => w,
        Err(e) => return (400, json!({"error": e})),
    };
    let kind = parse_kind(&detail["task_kind"]).unwrap_or(TaskKind::General);
    let permissions = detail["permissions"]
        .as_str()
        .and_then(TaskPermissions::parse)
        .unwrap_or_default();
    let model = detail["model"].as_str().map(str::to_owned);
    let billing =
        serde_json::from_value(detail["billing"].clone()).unwrap_or(AgentBilling::Unknown);
    let selection = ax::Selection {
        executor: executor_name.to_owned(),
        model: model.clone(),
        billing,
        billing_source: "continued".to_owned(),
        reason: format!("継続: {}", original.id),
        warnings: Vec::new(),
    };
    let request = TaskRequest {
        task_id: String::new(),
        kind,
        objective: instruction.to_owned(),
        success_criteria: Vec::new(),
        workspace,
        context: strings(&args["context"], LIST_ITEMS, ax::policy::CONTEXT_CHARS),
        constraints: Vec::new(),
        permissions,
        model,
        resume_session: Some(session.to_owned()),
    };
    // A write task continues in its own worktree, one task at a time.
    let reuse_worktree = match (
        detail["worktree"].as_str(),
        detail["branch"].as_str(),
        detail["base_commit"].as_str(),
    ) {
        (Some(path), Some(branch), Some(base)) => {
            let path = PathBuf::from(path);
            if !path.is_dir() {
                return (
                    409,
                    json!({"error": "この作業の worktree は反映または破棄済み"}),
                );
            }
            if worktree_busy(shared, &path.display().to_string()) {
                return (
                    409,
                    json!({"error": "同じ worktree で別のタスクが動いている"}),
                );
            }
            Some(Worktree {
                path,
                branch: branch.to_owned(),
                base: base.to_owned(),
            })
        }
        _ => None,
    };
    let start = Start {
        title: format!("継続: {instruction}"),
        request,
        workspace_name,
        selection,
        depends_on: vec![original.id.clone()],
        by: by.to_owned(),
        continues: Some(original.id.clone()),
        eval: None,
        shadow_choice: None,
        reuse_worktree,
    };
    match start_task(shared, &plane, start) {
        Ok(reply) => (200, reply),
        Err(e) => e,
    }
}

fn run_job(job: Job) {
    let Job {
        plane,
        board,
        spool,
        node,
        executor,
        selection,
        workspace_name,
        request,
        cancel,
        by,
        reservation,
        eval,
        shadow_choice,
        reuse_worktree,
    } = job;
    let mut request = request;
    let task_id = request.task_id.clone();
    let spec = executor.spec().clone();
    let finish = |status: TaskStatus, note: &str, detail: Value| {
        board.update(
            &spool,
            &task_id,
            Some(status),
            Some(("system", note)),
            Some(detail),
        );
        plane
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&task_id);
    };
    if !plane.acquire(&spec, &cancel) {
        plane.ledger.settle(&reservation, None, None, true);
        finish(TaskStatus::Cancelled, "開始前に取り消された", json!({}));
        return;
    }
    let started_at = unix_now();
    board.update(
        &spool,
        &task_id,
        Some(TaskStatus::InProgress),
        Some(("system", &format!("{} で開始", spec.name))),
        Some(json!({"started": iso8601(started_at)})),
    );
    // The workspace itself must come out of every task unchanged; a write
    // task works in its own worktree instead.
    let main_workspace = request.workspace.clone();
    let before = tree_state(&main_workspace);
    let writes = request.permissions != TaskPermissions::ReadOnly;
    let worktree = if writes {
        let made = match reuse_worktree {
            Some(wt) => Ok(wt),
            None => task_worktree::create(
                &main_workspace,
                &plane.worktrees_dir.join(&workspace_name),
                &task_id,
            ),
        };
        match made {
            Ok(wt) => {
                board.update(
                    &spool,
                    &task_id,
                    None,
                    Some(("system", &format!("worktree {} で作業", wt.branch))),
                    Some(json!({"worktree": wt.path.display().to_string(),
                                "branch": wt.branch, "base_commit": wt.base})),
                );
                request.workspace.clone_from(&wt.path);
                Some(wt)
            }
            Err(e) => {
                plane.release(&spec.name);
                plane.ledger.settle(&reservation, None, None, true);
                finish(
                    TaskStatus::Failed,
                    &format!("worktree を作れない: {e}"),
                    json!({}),
                );
                return;
            }
        }
    } else {
        None
    };
    let mut last_note = Instant::now();
    let mut tool_calls: u64 = 0;
    let mut on_event = |event: TaskEvent| match event {
        TaskEvent::Session { id } => {
            board.update(
                &spool,
                &task_id,
                None,
                None,
                Some(json!({"external_session_id": id})),
            );
        }
        TaskEvent::Tool { name, finished } => {
            if finished {
                tool_calls += 1;
            }
            if last_note.elapsed() >= NOTE_EVERY {
                last_note = Instant::now();
                board.update(
                    &spool,
                    &task_id,
                    None,
                    Some(("system", &format!("tool: {name}（完了 {tool_calls} 回）"))),
                    Some(json!({"tool_calls": tool_calls})),
                );
            }
        }
        TaskEvent::Note { text } => {
            if last_note.elapsed() >= NOTE_EVERY {
                last_note = Instant::now();
                board.update(&spool, &task_id, None, Some(("system", &text)), None);
            }
        }
    };
    let outcome: TaskOutcome = executor.run(&request, &cancel, &mut on_event);
    plane.release(&spec.name);
    let after = tree_state(&main_workspace);
    let (main_changes, tree_changed) = match (&before, &after) {
        (Some(b), Some(a)) => tree_changes(b, a),
        _ => (Vec::new(), false),
    };
    let violation = tree_changed;
    // A write task's result is its branch: commit what was left, keep the
    // patch next to the evidence.
    let (files_changed, diff_stat, patch_path, capture_error) = match &worktree {
        Some(wt) => match task_worktree::capture(
            wt,
            &format!("task {task_id}: {}", clip(&request.objective, 72)),
        ) {
            Ok(c) => {
                let patch = plane.results_dir.join(format!("{task_id}.patch"));
                let _ = std::fs::create_dir_all(&plane.results_dir);
                let _ = atomic_write(&patch, c.patch.as_bytes());
                (
                    c.files,
                    Some(c.diff_stat),
                    Some(patch.display().to_string()),
                    None,
                )
            }
            Err(e) => (Vec::new(), None, None, Some(e)),
        },
        None => (main_changes.clone(), None, None, None),
    };
    let finished_at = unix_now();
    let evidence_id = format!("ev-task-{task_id}");
    let status = if violation || capture_error.is_some() {
        TaskStatus::Failed
    } else {
        match outcome.end {
            RunEnd::Succeeded => TaskStatus::Done,
            RunEnd::Cancelled => TaskStatus::Cancelled,
            RunEnd::Failed | RunEnd::TimedOut => TaskStatus::Failed,
        }
    };
    // Cost: what the harness reported, else an estimate from listed prices
    // for metered models; otherwise unknown.
    let catalog = plane.catalog_snapshot();
    let entry = catalog.executors.get(&spec.name);
    let tokens = {
        let u = &outcome.usage;
        (!u.is_empty()).then(|| u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0))
    };
    #[allow(clippy::cast_precision_loss)]
    let estimated_usd = match (outcome.reported_cost_usd, selection.billing) {
        (None, AgentBilling::Metered) => selection
            .model
            .as_deref()
            .and_then(|m| entry.and_then(|e| e.find(m)))
            .and_then(|d| d.usd_per_mtok)
            .zip(outcome.usage.input_tokens.zip(outcome.usage.output_tokens))
            .map(|((pi, po), (ti, to))| (ti as f64 * pi + to as f64 * po) / 1_000_000.0),
        _ => None,
    };
    // Plan-covered work costs nothing extra; only metered/unknown usage is
    // money this ledger must account for.
    let ledger_usd = outcome
        .reported_cost_usd
        .or(estimated_usd)
        .or(match selection.billing {
            AgentBilling::Local
            | AgentBilling::FreeTier
            | AgentBilling::Subscription
            | AgentBilling::IncludedCredit => Some(0.0),
            AgentBilling::Metered | AgentBilling::Unknown => None,
        });
    plane.ledger.settle(&reservation, ledger_usd, tokens, false);
    let grade = eval.as_ref().map(|(run, case)| {
        ax::history::grade(
            run,
            case,
            status == TaskStatus::Done,
            &outcome.summary,
            files_changed.len(),
        )
    });
    let mut record = json!({
        "evidence_id": evidence_id,
        "kind": "external_task_result",
        // What an outside agent reported. Not testimony, not a belief.
        "trust": "external_content",
        "task_id": task_id,
        "status": status,
        "end": outcome.end,
        "objective": request.objective,
        "task_kind": request.kind.as_str(),
        "executor": spec.name,
        "adapter": spec.adapter.as_str(),
        "protocol": spec.protocol.as_str(),
        "model": selection.model,
        "model_ref": selection.model.as_deref().map(|m| format!("{}:{m}", spec.name)),
        "billing": selection.billing.as_str(),
        "session_id": outcome.session_id,
        "continued_session": request.resume_session,
        "summary": outcome.summary,
        "error": outcome.error,
        "files_changed": files_changed,
        "workspace_checked": before.is_some() && after.is_some(),
        // The workspace changed during the task: outside the worktree for a
        // write task, at all for a read-only one.
        "read_only_violation": violation,
        "tests": [],
        "tool_activity": outcome.tool_activity,
        "usage": outcome.usage,
        // `null` = not reported by the harness; never read as zero.
        "cost": {"reported_usd": outcome.reported_cost_usd, "estimated_usd": estimated_usd,
                 "billing": selection.billing.as_str()},
        "duration_ms": outcome.duration_ms,
        "exit_code": outcome.exit_code,
        "eval": grade,
        "provenance": {
            "node": node,
            "requested_by": by,
            "workspace": workspace_name,
            "permissions": request.permissions.as_str(),
            "routing": selection.reason,
            "shadow_routing": shadow_choice,
            "harness_version": entry.and_then(|c| c.harness_version.clone()),
            "started_at": iso8601(started_at),
            "finished_at": iso8601(finished_at),
        },
    });
    // Write tasks: where the work is.
    record["worktree"] = json!(worktree.as_ref().map(|w| w.path.display().to_string()));
    record["branch"] = json!(worktree.as_ref().map(|w| w.branch.clone()));
    record["base_commit"] = json!(worktree.as_ref().map(|w| w.base.clone()));
    record["diff_stat"] = json!(diff_stat);
    record["patch"] = json!(patch_path);
    plane.store_result(&spool, &record);
    if outcome.end != RunEnd::Cancelled {
        let history = HistoryRecord {
            task_id: task_id.clone(),
            at: finished_at,
            task_kind: request.kind,
            executor: spec.name.clone(),
            model: selection.model.clone(),
            billing: selection.billing,
            succeeded: status == TaskStatus::Done,
            duration_ms: outcome.duration_ms,
            tool_calls: outcome.tool_activity.len() as u64,
            tokens,
            usd: outcome.reported_cost_usd.or(estimated_usd),
            files_touched: files_changed.len(),
            eval: grade.clone(),
            shadow_choice: shadow_choice.clone(),
        };
        plane.history.record(&history);
        let _ = spool.append(
            "logs/task-history",
            serde_json::to_value(&history).unwrap_or(Value::Null),
        );
    }
    // Evaluation branches are graded, not applied.
    if let (Some(wt), Some(_)) = (&worktree, &eval) {
        let _ = task_worktree::remove(&main_workspace, &wt.path, &wt.branch, true);
    }
    let note = if violation {
        format!(
            "workspace 本体が変更された（隔離違反）: {}",
            main_changes.join(", ")
        )
    } else if let Some(error) = &capture_error {
        format!("変更を記録できない: {error}")
    } else if let Some(error) = &outcome.error {
        clip(error, 300)
    } else {
        clip(&outcome.summary, 300)
    };
    finish(
        status,
        &note,
        json!({
            "result_evidence_id": evidence_id,
            "duration_ms": outcome.duration_ms,
            "tool_calls": outcome.tool_activity.len(),
            "usage": outcome.usage,
            "reported_cost_usd": outcome.reported_cost_usd,
            "estimated_cost_usd": estimated_usd,
            "files_changed": files_changed,
            "diff_stat": diff_stat,
            "eval_passed": grade.as_ref().map(|g| g.passed),
        }),
    );
}

fn delegated(shared: &Shared, args: &Value) -> Result<crate::tasks::Task, (u16, Value)> {
    let id = args["id"]
        .as_str()
        .or_else(|| args["task_id"].as_str())
        .unwrap_or("")
        .trim();
    match shared.tasks.get(id) {
        Some(t) if t.kind == "delegated" => Ok(t),
        Some(_) => Err((400, json!({"error": format!("{id} は委譲タスクではない")}))),
        None => Err((404, json!({"error": format!("no task {id}")}))),
    }
}

/// `task_status`: progress, and the result once finished.
pub fn status(shared: &Shared, args: &Value) -> (u16, Value) {
    let plane = match plane(shared) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let task = match delegated(shared, args) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let view = task.view();
    let mut reply = json!({
        "task_id": task.id,
        "status": task.status,
        "title": task.title,
        "executor": view["detail"]["executor"],
        "model": view["detail"]["model"],
        "billing": view["detail"]["billing"],
        "created": view["created"],
        "duration_secs": view["duration_secs"],
        "notes": task.notes.iter().rev().take(5).rev().map(|n| json!({
            "at": iso8601(n.at), "by": n.by, "text": n.text})).collect::<Vec<_>>(),
    });
    if let Some(result) = plane.load_result(&task.id) {
        reply["result"] = json!({
            "evidence_id": result["evidence_id"],
            "summary": clip(result["summary"].as_str().unwrap_or(""), STATUS_SUMMARY_CHARS),
            "error": result["error"],
            "files_changed": result["files_changed"],
            "read_only_violation": result["read_only_violation"],
            "branch": result["branch"],
            "diff_stat": result["diff_stat"],
            "usage": result["usage"],
            "cost": result["cost"],
            "session_id": result["session_id"],
            "duration_ms": result["duration_ms"],
        });
        reply["note"] = json!(
            "外部エージェントの報告であり確定した事実ではない。必要なら根拠を確認してから使う"
        );
    }
    (200, reply)
}

/// `task_cancel`: stop a queued or running delegated task.
pub fn cancel(shared: &Shared, args: &Value, by: &str) -> (u16, Value) {
    let plane = match plane(shared) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let task = match delegated(shared, args) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let flag = plane
        .running
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&task.id)
        .cloned();
    let Some(flag) = flag else {
        return (
            409,
            json!({"error": format!("{} は実行中ではない（{:?}）", task.id, task.status)}),
        );
    };
    flag.store(true, Ordering::Relaxed);
    shared.tasks.update(
        &shared.spool,
        &task.id,
        None,
        Some((by, "取り消しを要求")),
        None,
    );
    (200, json!({"task_id": task.id, "cancelling": true}))
}

/// `agent_models`: the cached catalog, filtered. Never blocks on discovery.
pub fn models(shared: &Shared, args: &Value) -> (u16, Value) {
    let plane = match plane(shared) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let registry = plane.registry();
    let catalog = plane.catalog_snapshot();
    let only = args["executor"]
        .as_str()
        .filter(|e| !e.is_empty() && *e != "auto");
    let query = args["query"]
        .as_str()
        .map(str::to_lowercase)
        .filter(|q| !q.is_empty());
    let billing = args["billing"].as_str().filter(|b| !b.is_empty());
    let auto_only = args["auto_eligible"].as_bool().unwrap_or(false);
    let limit = usize::try_from(args["limit"].as_u64().unwrap_or(40).clamp(1, 200)).unwrap_or(40);
    let mut executors = Vec::new();
    let mut models = Vec::new();
    let mut matched = 0usize;
    let mut pending = false;
    for executor in registry.all() {
        let spec = executor.spec();
        if only.is_some_and(|o| o != spec.name) {
            continue;
        }
        let health = executor.health();
        let entry = catalog.executors.get(&spec.name);
        if entry.is_none() && health.ok {
            pending = true;
        }
        executors.push(json!({
            "executor": spec.name,
            "adapter": spec.adapter.as_str(),
            "ok": health.ok,
            "detail": health.detail,
            "default_model": spec.default_model,
            "default_billing": spec.default_billing.as_str(),
            "prefer_for": spec.prefer_for,
            "models": entry.map(|e| e.models.len()),
            "checked": entry.filter(|e| e.checked_at > 0).map(|e| iso8601(e.checked_at)),
            "error": entry.and_then(|e| e.error.clone()),
        }));
        for m in entry.into_iter().flat_map(|e| &e.models) {
            if billing.is_some_and(|b| b != m.billing.as_str())
                || (auto_only && !m.billing.auto_eligible())
            {
                continue;
            }
            if let Some(q) = &query {
                let hay = format!(
                    "{} {} {}",
                    m.id.model,
                    m.aliases.join(" "),
                    m.label.as_deref().unwrap_or("")
                )
                .to_lowercase();
                if !hay.contains(q.as_str()) {
                    continue;
                }
            }
            matched += 1;
            if models.len() >= limit {
                continue;
            }
            let caps = m.capabilities;
            let tags: Vec<&str> = [
                (caps.reasoning, "reasoning"),
                (caps.vision, "vision"),
                (caps.tools, "tools"),
                (caps.long_context, "long_context"),
                (caps.fast, "fast"),
            ]
            .into_iter()
            .filter_map(|(on, tag)| on.then_some(tag))
            .collect();
            models.push(json!({
                "ref": m.id.to_string(),
                "aliases": m.aliases,
                "label": m.label,
                "billing": m.billing.as_str(),
                "context_tokens": m.context_tokens,
                "capabilities": tags,
                "available": m.available,
            }));
        }
    }
    if pending {
        plane.request_refresh();
    }
    (
        200,
        json!({
            "executors": executors,
            "models": models,
            "matched": matched,
            "truncated": matched > models.len(),
            "discovery_pending": pending,
            "hint": "task_delegate の model には ref（executor:model）をそのまま渡せる。metered/unknown は自動選択されない",
        }),
    )
}

/// Whether a queued or running task works in `worktree`.
fn worktree_busy(shared: &Shared, worktree: &str) -> bool {
    shared.tasks.list(false).iter().any(|t| {
        t["kind"] == "delegated"
            && t["detail"]["worktree"].as_str() == Some(worktree)
            && matches!(t["status"].as_str(), Some("waiting" | "in_progress"))
    })
}

/// `apply` / `discard` (operator): merge a finished write task's branch
/// into its workspace (`--no-ff`, clean tree only), or drop it.
pub fn worktree_action(shared: &Shared, args: &Value, by: &str, apply: bool) -> (u16, Value) {
    let plane = match plane(shared) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if by != "operator" {
        return (403, json!({"error": "反映・破棄は操作者だけが行う"}));
    }
    let task = match delegated(shared, args) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let (Some(path), Some(branch)) = (
        task.detail["worktree"].as_str(),
        task.detail["branch"].as_str(),
    ) else {
        return (400, json!({"error": "書き込みタスクではない"}));
    };
    if !matches!(
        task.status,
        TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled
    ) {
        return (409, json!({"error": "タスクがまだ終わっていない"}));
    }
    if worktree_busy(shared, path) {
        return (
            409,
            json!({"error": "この worktree で続きのタスクが動いている"}),
        );
    }
    let (_, workspace) = match plane.workspace(task.detail["workspace"].as_str()) {
        Ok(w) => w,
        Err(e) => return (400, json!({"error": e})),
    };
    let path = PathBuf::from(path);
    if !path.is_dir() {
        return (409, json!({"error": "既に反映または破棄されている"}));
    }
    let merged = if apply {
        match task_worktree::apply(
            &workspace,
            branch,
            &format!("Merge task {} ({})", task.id, clip(&task.title, 60)),
        ) {
            Ok(commit) => Some(commit),
            Err(e) => return (409, json!({"error": e})),
        }
    } else {
        None
    };
    // After a merge the branch is part of the workspace history; after a
    // discard it goes. The patch stays with the evidence either way.
    if let Err(e) = task_worktree::remove(&workspace, &path, branch, true) {
        return (500, json!({"error": e}));
    }
    let note = match &merged {
        Some(commit) => format!("workspace に反映: {}", &commit[..commit.len().min(12)]),
        None => "worktree とブランチを破棄".to_owned(),
    };
    shared.tasks.update(
        &shared.spool,
        &task.id,
        None,
        Some((by, &note)),
        Some(json!({"applied": merged, "discarded": merged.is_none()})),
    );
    (
        200,
        json!({"ok": true, "task_id": task.id, "merged": merged}),
    )
}

/// `feedback` (operator): accept or reject a result; history-based
/// routing weighs it above the harness's own success report.
pub fn feedback(shared: &Shared, args: &Value, by: &str) -> (u16, Value) {
    let plane = match plane(shared) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if by != "operator" {
        return (403, json!({"error": "feedback is the operator's"}));
    }
    let task = match delegated(shared, args) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let Some(verdict) = args["verdict"].as_str().and_then(Verdict::parse) else {
        return (400, json!({"error": "verdict must be accept or reject"}));
    };
    let note = args["note"].as_str().map(|n| clip(n, ITEM_CHARS));
    plane.history.feedback(&Feedback {
        task_id: task.id.clone(),
        at: unix_now(),
        verdict,
        note: note.clone(),
    });
    let text = format!(
        "{}{}",
        if verdict == Verdict::Accept {
            "採用"
        } else {
            "不採用"
        },
        note.map(|n| format!(": {n}")).unwrap_or_default()
    );
    shared.tasks.update(
        &shared.spool,
        &task.id,
        None,
        Some((by, &text)),
        Some(json!({"feedback": verdict})),
    );
    (
        200,
        json!({"ok": true, "task_id": task.id, "verdict": verdict}),
    )
}

fn stats_view(plane: &TaskPlane) -> Vec<Value> {
    let (records, feedback) = plane.history.load();
    ax::history::stats(&records, &feedback)
        .into_iter()
        .map(|row| serde_json::to_value(row).unwrap_or(Value::Null))
        .collect()
}

/// `stats`: outcomes per task kind × executor × model.
pub fn stats(shared: &Shared) -> (u16, Value) {
    match plane(shared) {
        Ok(p) => (
            200,
            json!({"routing_mode": p.config.routing_mode.as_str(),
                   "min_samples": p.config.history_min_samples,
                   "stats": stats_view(p)}),
        ),
        Err(e) => e,
    }
}

fn battery(plane: &TaskPlane) -> Result<Battery, String> {
    match &plane.config.battery_file {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("{}: {e}", path.display()))
            .and_then(|t| Battery::parse(&t)),
        None => Ok(Battery::default_battery()),
    }
}

/// `eval` (operator): run the battery on each target (`executor` or
/// `executor:model`), as ordinary queued, quota-checked tasks.
pub fn eval(shared: &Shared, args: &Value, by: &str) -> (u16, Value) {
    let plane = match plane(shared) {
        Ok(p) => Arc::clone(p),
        Err(e) => return e,
    };
    if by != "operator" {
        return (
            403,
            json!({"error": "evaluation runs are started by the operator"}),
        );
    }
    let battery = match battery(&plane) {
        Ok(b) => b,
        Err(e) => return (400, json!({"error": e})),
    };
    let targets: Vec<String> = strings(&args["targets"], 20, 200);
    if targets.is_empty() {
        return (
            400,
            json!({"error": "targets: [\"executor[:model]\", …] is required"}),
        );
    }
    let only: Vec<String> = strings(&args["cases"], 50, 40);
    let (workspace_name, workspace) = match plane.workspace(args["workspace"].as_str()) {
        Ok(w) => w,
        Err(e) => return (400, json!({"error": e})),
    };
    let writable = plane.allows_write(&workspace_name) && task_worktree::is_repo(&workspace);
    let run_id = format!("e{:x}", crate::util::unix_now_ms());
    let registry = plane.registry();
    let pool = candidates(&registry);
    let cands: Vec<ax::Candidate<'_>> = pool
        .iter()
        .map(|(e, ok)| ax::Candidate {
            spec: e.spec(),
            healthy: *ok,
        })
        .collect();
    let catalog = plane.catalog_snapshot();
    let (mut started, mut skipped, mut errors) = (Vec::new(), Vec::new(), Vec::new());
    for target in &targets {
        let (executor, model) = match target.split_once(':') {
            Some((e, m)) => (e, Some(m)),
            None => (target.as_str(), None),
        };
        for case in battery
            .cases
            .iter()
            .filter(|c| only.is_empty() || only.contains(&c.id))
        {
            if case.requires_write && !writable {
                skipped.push(json!({"target": target, "case": case.id,
                                    "reason": "workspace does not allow write tasks"}));
                continue;
            }
            let selection = match ax::select(case.kind, Some(executor), model, &cands, &catalog) {
                Ok(s) => s,
                Err(e) => {
                    errors.push(json!({"target": target, "case": case.id, "error": e}));
                    continue;
                }
            };
            let request = TaskRequest {
                task_id: String::new(),
                kind: case.kind,
                objective: case.objective.clone(),
                success_criteria: case.success_criteria.clone(),
                workspace: workspace.clone(),
                context: Vec::new(),
                constraints: Vec::new(),
                permissions: if case.requires_write {
                    TaskPermissions::WorkspaceWrite
                } else {
                    TaskPermissions::ReadOnly
                },
                model: selection.model.clone(),
                resume_session: None,
            };
            let start = Start {
                title: format!("評価 {} {}: {target}", run_id, case.id),
                request,
                workspace_name: workspace_name.clone(),
                selection,
                depends_on: Vec::new(),
                by: by.to_owned(),
                continues: None,
                eval: Some((run_id.clone(), case.clone())),
                shadow_choice: None,
                reuse_worktree: None,
            };
            match start_task(shared, &plane, start) {
                Ok(reply) => started.push(json!({"target": target, "case": case.id,
                                                  "task_id": reply["task_id"]})),
                Err((_, e)) => errors.push(json!({"target": target, "case": case.id,
                                                   "error": e["error"]})),
            }
        }
    }
    (
        200,
        json!({"run_id": run_id, "battery": battery.name, "tasks": started,
               "skipped": skipped, "errors": errors}),
    )
}

/// `eval_report`: per-target results of one evaluation run.
pub fn eval_report(shared: &Shared, args: &Value) -> (u16, Value) {
    let plane = match plane(shared) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let run_id = args["run_id"].as_str().unwrap_or("");
    let (records, _) = plane.history.load();
    let pending = shared
        .tasks
        .list(true)
        .iter()
        .filter(|t| {
            t["detail"]["eval"]["run_id"].as_str() == Some(run_id)
                && !matches!(t["status"].as_str(), Some("done" | "failed" | "cancelled"))
        })
        .count();
    (
        200,
        json!({"run_id": run_id, "pending": pending,
               "targets": ax::history::eval_report(run_id, &records)}),
    )
}

/// `panel`: everything the desktop Task Plane panel shows, in one call.
pub fn panel(shared: &Shared) -> (u16, Value) {
    let plane = match plane(shared) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let (_, feedback) = plane.history.load();
    let verdicts: HashMap<&str, Verdict> = feedback
        .iter()
        .map(|f| (f.task_id.as_str(), f.verdict))
        .collect();
    let mut tasks: Vec<Value> = shared
        .tasks
        .list(false)
        .into_iter()
        .filter(|t| t["kind"] == "delegated")
        .collect();
    tasks.reverse();
    tasks.truncate(100);
    for task in &mut tasks {
        if let Some(notes) = task["notes"].as_array_mut() {
            let keep = notes.len().saturating_sub(5);
            notes.drain(..keep);
        }
        let id = task["id"].as_str().unwrap_or("").to_owned();
        task["result"] = plane.load_result(&id).map_or(Value::Null, |r| {
            let tools = r["tool_activity"].as_array().cloned().unwrap_or_default();
            let tail = tools[tools.len().saturating_sub(30)..].to_vec();
            json!({
                "summary": clip(r["summary"].as_str().unwrap_or(""), 2_000),
                "error": r["error"],
                "usage": r["usage"],
                "cost": r["cost"],
                "files_changed": r["files_changed"],
                "tool_activity": tail,
                "session_id": r["session_id"],
                "evidence_id": r["evidence_id"],
                "read_only_violation": r["read_only_violation"],
                "diff_stat": r["diff_stat"],
                "branch": r["branch"],
                "eval": r["eval"],
                "feedback": verdicts.get(id.as_str()),
            })
        });
    }
    let mut quotas: std::collections::BTreeMap<String, ax::Quota> = plane
        .registry()
        .all()
        .iter()
        .filter(|e| !e.spec().quota.is_empty())
        .map(|e| (e.spec().name.clone(), e.spec().quota.clone()))
        .collect();
    if let Some(q) = &plane.config.quota {
        quotas.insert(TOTAL.to_owned(), q.clone());
    }
    (
        200,
        json!({
            "health": plane.status(),
            "tasks": tasks,
            "ledger": plane.ledger.view(unix_now(), &quotas),
            "stats": stats_view(plane),
        }),
    )
}

/// `agent_health`.
pub fn health(shared: &Shared) -> (u16, Value) {
    match plane(shared) {
        Ok(p) => (200, p.status()),
        Err(e) => e,
    }
}

/// `POST /v1/agents` (operator) and the individual's tools.
pub fn handle(shared: &Shared, request: &Value, by: &str) -> (u16, Value) {
    match request["action"].as_str().unwrap_or("health") {
        "health" => health(shared),
        "models" => models(shared, request),
        "refresh" => match plane(shared) {
            Ok(p) => {
                p.request_refresh();
                (200, json!({"ok": true, "refreshing": true}))
            }
            Err(e) => e,
        },
        "delegate" => delegate(shared, request, by),
        "continue" => continue_task(shared, request, by),
        "status" => status(shared, request),
        "cancel" => cancel(shared, request, by),
        "feedback" => feedback(shared, request, by),
        "apply" => worktree_action(shared, request, by, true),
        "discard" => worktree_action(shared, request, by, false),
        "stats" => stats(shared),
        "eval" => eval(shared, request, by),
        "eval_report" => eval_report(shared, request),
        "panel" => panel(shared),
        other => (400, json!({"error": format!("unknown action {other}")})),
    }
}

/// Tool-call entry point: wraps replies in the tools' `ok`/`error` shape.
pub fn tool(shared: &Shared, name: &str, args: &Value) -> (u16, Value) {
    let action = match name {
        "task_delegate" => "delegate",
        "task_status" => "status",
        "task_cancel" => "cancel",
        "task_continue" => "continue",
        "agent_models" => "models",
        _ => "health",
    };
    let mut request = if args.is_object() {
        args.clone()
    } else {
        json!({})
    };
    request["action"] = json!(action);
    let (status, reply) = handle(shared, &request, "mio");
    if status == 200 {
        (200, json!({"ok": true, "result": reply}))
    } else {
        (
            200,
            json!({"ok": false, "error": reply.get("error").cloned().unwrap_or(reply)}),
        )
    }
}

/// Tool definitions (kept small: the individual needs intent, not CLI
/// details).
pub fn definitions() -> Vec<Value> {
    let f = |name: &str, description: &str, parameters: Value| {
        json!({"type": "function", "function": {
            "name": name, "description": description, "parameters": parameters}})
    };
    vec![
        f(
            "task_delegate",
            "Hand a longer piece of work (research, code reading, review, debugging, planning, code changes) to an external agent (Devin / OpenCode / Command Code …). Returns a task id immediately; do not wait — check later with task_status. Read-only unless permissions=workspace_write (isolated branch, merged only by the operator).",
            json!({"type": "object", "properties": {
                "objective": {"type": "string", "description": "what to find out or do, self-contained"},
                "kind": {"type": "string", "enum": ["research", "coding", "review", "debug", "planning", "background", "general"]},
                "workspace": {"type": "string", "description": "allowed workspace name (agent_health lists them)"},
                "executor": {"type": "string", "description": "auto (default) or an executor name, e.g. opencode"},
                "model": {"type": "string", "description": "model id, or executor:model from agent_models"},
                "success_criteria": {"type": "array", "items": {"type": "string"}},
                "context": {"type": "string", "description": "excerpts the agent needs; never private memories or credentials"},
                "permissions": {"type": "string", "enum": ["read_only", "workspace_write"],
                                "description": "workspace_write runs in an isolated git worktree/branch; the operator decides whether to merge it"}},
                "required": ["objective"]}),
        ),
        f(
            "task_status",
            "Status of a delegated task; once finished, the external agent's report (evidence, not established fact).",
            json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        ),
        f(
            "task_cancel",
            "Cancel a queued or running delegated task.",
            json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        ),
        f(
            "task_continue",
            "Give a finished delegated task a follow-up instruction in the same external session (same agent, model and workspace). Returns a new task id at once.",
            json!({"type": "object", "properties": {
                "id": {"type": "string", "description": "the finished task"},
                "instruction": {"type": "string"}},
                "required": ["id", "instruction"]}),
        ),
        f(
            "agent_models",
            "List models the external agents can run, as executor:model refs with billing (local/free_tier/subscription/included_credit/metered/unknown).",
            json!({"type": "object", "properties": {
                "executor": {"type": "string"},
                "query": {"type": "string", "description": "substring of model id/name"},
                "auto_eligible": {"type": "boolean", "description": "only models routing may pick on its own"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}}}),
        ),
        f(
            "agent_health",
            "Which external agents are available, running tasks, and allowed workspaces.",
            json!({"type": "object", "properties": {}}),
        ),
    ]
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script");
        let mut perms = std::fs::metadata(&path).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).expect("chmod");
        path
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git")
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A resident with one fake harness. The harness lists `fake-1`,
    /// answers with its arguments, `sleep`s when told to, and writes a
    /// file when told to.
    fn fixture(executors_file: bool) -> (Shared, tempfile::TempDir) {
        fixture_with(executors_file, &json!({}), &json!({}))
    }

    /// `plane_extra` / `executor_extra` are merged into the task plane and
    /// the fake executor configuration.
    fn fixture_with(
        executors_file: bool,
        plane_extra: &Value,
        executor_extra: &Value,
    ) -> (Shared, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).expect("ws");
        git(&ws, &["init", "-q"]);
        git(
            &ws,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        // Lists models; otherwise answers in opencode's JSON event format
        // with a session id, echoing the model, the resumed session and
        // whether a resident secret leaked into its environment.
        let agent = script(
            dir.path(),
            "fake-agent",
            "if [ \"$1\" = \"--list\" ]; then printf 'fake-1  first\\nfake-2\\nfake-metered\\n'; exit 0; fi\n\
             model=none; session=none\n\
             while [ $# -gt 1 ]; do case \"$1\" in --model) model=$2; shift;; --session) session=$2; shift;; esac; shift; done\n\
             case \"$1\" in *SLEEP*) sleep 30;; esac\n\
             case \"$1\" in *WRITE*) echo x > written.txt;; esac\n\
             printf '{\"type\":\"text\",\"sessionID\":\"s-1\",\"part\":{\"text\":\"model=%s session=%s secret=%s\"}}\\n' \
               \"$model\" \"$session\" \"${CARGO_MANIFEST_DIR:-none}\"",
        );
        let mut executor = json!({
            "name": "fake", "adapter": "generic", "command": agent,
            "discover": {"args": ["--list"], "format": "lines"},
            "run": {"args": ["{prompt}"], "model_args": ["--model", "{model}"],
                    "resume_args": ["--session", "{session}"], "output": "opencode_json"},
            "permission_args": {"read_only": []},
            "default_billing": "local", "default_model": "fake-1",
            "billing": [{"models": "fake-metered", "billing": "metered"}],
            "prefer_for": ["research"]
        });
        for (k, v) in executor_extra.as_object().into_iter().flatten() {
            executor[k] = v.clone();
        }
        let mut plane = json!({
            "workspaces": [{"name": "ws", "path": ws}],
            "check_interval_secs": 3600
        });
        for (k, v) in plane_extra.as_object().into_iter().flatten() {
            if k == "allow_write" {
                plane["workspaces"][0]["allow_write"] = v.clone();
            } else {
                plane[k] = v.clone();
            }
        }
        if executors_file {
            let file = dir.path().join("executors.json");
            std::fs::write(&file, json!({"executors": [executor]}).to_string()).expect("file");
            plane["executors_file"] = json!(file);
        } else {
            plane["executors"] = json!([executor]);
        }
        let config = serde_json::from_value(json!({
            "node": {"id": "test", "role": "cognition", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()},
            "task_plane": plane
        }))
        .expect("config");
        let config: crate::config::Config = config;
        config.validate().expect("valid");
        let spool = Spool::new(dir.path().join("spool"), "test").expect("spool");
        let shared = Shared::new(config, spool, 1, None);
        shared.task_plane.as_ref().expect("plane").refresh_catalog();
        (shared, dir)
    }

    fn delegate_ok(shared: &Shared, args: Value) -> String {
        let (_, reply) = tool(shared, "task_delegate", &args);
        assert_eq!(reply["ok"], true, "{reply}");
        reply["result"]["task_id"].as_str().expect("id").to_owned()
    }

    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    #[test]
    fn write_tasks_run_in_their_own_worktree_and_are_merged_only_on_request() {
        let writable = json!({"permission_args": {"read_only": [], "workspace_write": [],
                                                  "autonomous_workspace": []}});
        // Not allowed on a read-only workspace.
        let (shared, _dir) = fixture_with(false, &json!({}), &writable);
        let (_, reply) = tool(
            &shared,
            "task_delegate",
            &json!({"objective": "WRITE", "permissions": "workspace_write"}),
        );
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or("")
                .contains("allow_write"),
            "{reply}"
        );

        let (shared, dir) = fixture_with(false, &json!({"allow_write": true}), &writable);
        let ws = dir.path().join("ws");
        let (_, reply) = tool(
            &shared,
            "task_delegate",
            &json!({"objective": "x", "permissions": "autonomous_workspace"}),
        );
        assert_eq!(reply["ok"], false, "autonomous is the operator's call");

        let id = delegate_ok(
            &shared,
            json!({"objective": "WRITE", "permissions": "workspace_write"}),
        );
        let task = wait_finished(&shared, &id);
        assert_eq!(task.status, TaskStatus::Done, "{task:?}");
        assert!(
            !ws.join("written.txt").exists(),
            "the workspace is untouched"
        );
        let branch = task.detail["branch"].as_str().expect("branch").to_owned();
        let worktree = PathBuf::from(task.detail["worktree"].as_str().expect("worktree"));
        assert!(worktree.join("written.txt").exists());
        assert_eq!(task.detail["files_changed"], json!(["written.txt"]));
        let (_, status) = tool(&shared, "task_status", &json!({"id": id}));
        assert!(
            status["result"]["result"]["diff_stat"]
                .as_str()
                .unwrap_or("")
                .contains("written.txt")
        );
        assert_eq!(status["result"]["result"]["read_only_violation"], false);
        let patch = std::fs::read_to_string(
            dir.path()
                .join("current_state/task-results")
                .join(format!("{id}.patch")),
        )
        .expect("patch");
        assert!(patch.contains("+x"));

        // Continuing works in the same worktree and branch.
        let (_, reply) = tool(
            &shared,
            "task_continue",
            &json!({"id": id, "instruction": "続けて"}),
        );
        let next = reply["result"]["task_id"]
            .as_str()
            .expect("next")
            .to_owned();
        let cont = wait_finished(&shared, &next);
        assert_eq!(cont.detail["branch"], json!(branch));

        // The individual cannot merge; the operator can, into a clean tree.
        let (status, _) = handle(&shared, &json!({"action": "apply", "id": id}), "mio");
        assert_eq!(status, 403);
        let (status, reply) = handle(&shared, &json!({"action": "apply", "id": id}), "operator");
        assert_eq!(status, 200, "{reply}");
        assert!(ws.join("written.txt").exists(), "merged");
        assert!(!worktree.exists());
        assert!(git_out(&ws, &["branch", "--list", &branch]).is_empty());
        let (status, _) = handle(&shared, &json!({"action": "discard", "id": id}), "operator");
        assert_eq!(status, 409, "already applied");

        // Discard drops another task's branch without touching the workspace.
        let other = delegate_ok(
            &shared,
            json!({"objective": "WRITE again", "permissions": "workspace_write"}),
        );
        let t = wait_finished(&shared, &other);
        let other_branch = t.detail["branch"].as_str().expect("branch").to_owned();
        let head = git_out(&ws, &["rev-parse", "HEAD"]);
        let (status, _) = handle(
            &shared,
            &json!({"action": "discard", "id": other}),
            "operator",
        );
        assert_eq!(status, 200);
        assert!(git_out(&ws, &["branch", "--list", &other_branch]).is_empty());
        assert_eq!(git_out(&ws, &["rev-parse", "HEAD"]), head);
    }

    #[test]
    fn a_finished_task_continues_in_the_same_session() {
        let (shared, _dir) = fixture(false);
        let first = delegate_ok(&shared, json!({"objective": "調べて"}));
        wait_finished(&shared, &first);
        let (_, reply) = tool(
            &shared,
            "task_continue",
            &json!({"id": first, "instruction": "もう少し詳しく"}),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        let next = reply["result"]["task_id"].as_str().expect("id").to_owned();
        let task = wait_finished(&shared, &next);
        assert_eq!(task.status, TaskStatus::Done, "{task:?}");
        assert_eq!(task.depends_on, vec![first.clone()]);
        assert_eq!(task.detail["continues"], json!(first));
        let (_, status) = tool(&shared, "task_status", &json!({"id": next}));
        let summary = status["result"]["result"]["summary"]
            .as_str()
            .expect("summary");
        assert!(summary.contains("session=s-1"), "{summary}");
        assert!(summary.contains("model=fake-1"));
        // A running task cannot be continued.
        let running = delegate_ok(&shared, json!({"objective": "SLEEP"}));
        let (_, busy) = tool(
            &shared,
            "task_continue",
            &json!({"id": running, "instruction": "x"}),
        );
        assert_eq!(busy["ok"], false);
        tool(&shared, "task_cancel", &json!({"id": running}));
        wait_finished(&shared, &running);
    }

    #[test]
    fn quotas_and_metered_budgets_are_enforced() {
        let (shared, _dir) = fixture_with(
            false,
            &json!({}),
            &json!({"quota": {"max_tasks_per_day": 1}, "allow_metered": true}),
        );
        let first = delegate_ok(&shared, json!({"objective": "one"}));
        let (_, second) = tool(&shared, "task_delegate", &json!({"objective": "two"}));
        assert_eq!(second["ok"], false);
        assert!(
            second["error"].as_str().unwrap_or("").contains("1/1"),
            "{second}"
        );
        wait_finished(&shared, &first);
        let (_, panel) = handle(&shared, &json!({"action": "panel"}), "operator");
        assert_eq!(panel["ledger"]["today"]["fake"]["tasks"], 1);
        assert_eq!(
            panel["ledger"]["today"]["fake"]["unpriced"], 0,
            "local work costs nothing"
        );
        assert_eq!(panel["ledger"]["quotas"]["fake"]["max_tasks_per_day"], 1);
        // Metered needs an explicit spending budget even with allow_metered.
        let (shared, _dir) = fixture_with(false, &json!({}), &json!({"allow_metered": true}));
        let (_, reply) = tool(
            &shared,
            "task_delegate",
            &json!({"objective": "x", "model": "fake:fake-metered"}),
        );
        assert!(
            reply["error"].as_str().unwrap_or("").contains("費用上限"),
            "{reply}"
        );
    }

    #[test]
    fn evaluation_runs_grade_and_report() {
        let dir = tempfile::tempdir().expect("battery dir");
        let battery = dir.path().join("battery.json");
        std::fs::write(
            &battery,
            json!({"name": "t", "cases": [
                {"id": "A", "kind": "research", "objective": "a", "must_contain_all": ["model=fake-2"]},
                {"id": "B", "kind": "research", "objective": "b", "must_contain_all": ["nope"]},
                {"id": "W", "kind": "coding", "objective": "w", "requires_write": true}
            ]})
            .to_string(),
        )
        .expect("battery");
        let (shared, _dir) = fixture_with(false, &json!({"battery_file": battery}), &json!({}));
        let (status, run) = handle(
            &shared,
            &json!({"action": "eval", "targets": ["fake:fake-2"]}),
            "operator",
        );
        assert_eq!(status, 200, "{run}");
        assert_eq!(run["tasks"].as_array().expect("tasks").len(), 2);
        assert_eq!(run["skipped"][0]["case"], "W");
        for t in run["tasks"].as_array().expect("tasks") {
            wait_finished(&shared, t["task_id"].as_str().expect("id"));
        }
        let (_, report) = handle(
            &shared,
            &json!({"action": "eval_report", "run_id": run["run_id"]}),
            "operator",
        );
        let target = &report["targets"][0];
        assert_eq!(target["target"], "fake:fake-2");
        assert_eq!(
            (target["cases"].as_u64(), target["passed"].as_u64()),
            (Some(2), Some(1))
        );
        assert_eq!(target["failed_cases"], json!(["B"]));
        // The individual cannot start evaluations.
        let (status, _) = handle(
            &shared,
            &json!({"action": "eval", "targets": ["fake"]}),
            "mio",
        );
        assert_eq!(status, 403);
    }

    #[test]
    fn history_routing_uses_measured_outcomes_and_shadow_only_records() {
        for mode in ["history", "shadow"] {
            let (shared, _dir) = fixture_with(
                false,
                &json!({"routing_mode": mode, "history_min_samples": 2}),
                &json!({}),
            );
            let plane = shared.task_plane.as_ref().expect("plane");
            for i in 0..2 {
                plane.history.record(&HistoryRecord {
                    task_id: format!("h{i}"),
                    at: 1,
                    task_kind: TaskKind::Research,
                    executor: "fake".to_owned(),
                    model: Some("fake-2".to_owned()),
                    billing: AgentBilling::Local,
                    succeeded: true,
                    duration_ms: 10,
                    tool_calls: 0,
                    tokens: None,
                    usd: None,
                    files_touched: 0,
                    eval: None,
                    shadow_choice: None,
                });
            }
            let id = delegate_ok(&shared, json!({"objective": "x", "kind": "research"}));
            let task = shared.tasks.get(&id).expect("task");
            if mode == "history" {
                assert_eq!(task.detail["model"], "fake-2");
                assert!(
                    task.detail["routing"]
                        .as_str()
                        .unwrap_or("")
                        .starts_with("実績")
                );
            } else {
                assert_eq!(
                    task.detail["model"], "fake-1",
                    "shadow keeps static routing"
                );
                assert_eq!(task.detail["shadow_routing"], "fake:fake-2");
            }
            wait_finished(&shared, &id);
            // Feedback is the operator's and shows in stats.
            let (status, _) = handle(
                &shared,
                &json!({"action": "feedback", "id": id, "verdict": "reject"}),
                "mio",
            );
            assert_eq!(status, 403);
            let (status, _) = handle(
                &shared,
                &json!({"action": "feedback", "id": id, "verdict": "reject"}),
                "operator",
            );
            assert_eq!(status, 200);
            let (_, stats) = handle(&shared, &json!({"action": "stats"}), "operator");
            let rejected: u64 = stats["stats"]
                .as_array()
                .expect("rows")
                .iter()
                .filter_map(|r| r["rejected"].as_u64())
                .sum();
            assert_eq!(rejected, 1);
            let (_, panel) = handle(&shared, &json!({"action": "panel"}), "operator");
            assert_eq!(panel["tasks"][0]["result"]["feedback"], "reject");
            assert!(
                panel["health"]["executors"][0]["can_resume"]
                    .as_bool()
                    .expect("bool")
            );
        }
    }

    fn wait_finished(shared: &Shared, id: &str) -> crate::tasks::Task {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let task = shared.tasks.get(id).expect("task");
            if matches!(
                task.status,
                TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled
            ) {
                return task;
            }
            assert!(
                Instant::now() < deadline,
                "task {id} did not finish: {task:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn delegation_returns_at_once_and_the_result_becomes_evidence() {
        let (shared, _dir) = fixture(false);
        *shared.current_turn.lock().expect("turn") = Some("tturn".to_owned());
        let (status, models) = tool(&shared, "agent_models", &json!({}));
        assert_eq!(status, 200);
        let refs: Vec<&str> = models["result"]["models"]
            .as_array()
            .expect("models")
            .iter()
            .filter_map(|m| m["ref"].as_str())
            .collect();
        assert_eq!(
            refs,
            vec!["fake:fake-1", "fake:fake-2", "fake:fake-metered"]
        );

        let started = Instant::now();
        let (_, reply) = tool(
            &shared,
            "task_delegate",
            &json!({"objective": "調べて", "kind": "research"}),
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "chat must not wait"
        );
        assert_eq!(reply["ok"], true, "{reply}");
        let id = reply["result"]["task_id"].as_str().expect("id").to_owned();
        assert_eq!(reply["result"]["executor"], "fake");
        assert_eq!(reply["result"]["billing"], "local");

        let task = wait_finished(&shared, &id);
        assert_eq!(task.status, TaskStatus::Done, "{task:?}");
        assert_eq!(task.kind, "delegated");
        assert_eq!(task.depends_on, vec!["tturn"]);
        let (_, status) = tool(&shared, "task_status", &json!({"id": id}));
        let result = &status["result"]["result"];
        assert_eq!(result["evidence_id"], format!("ev-task-{id}"));
        let summary = result["summary"].as_str().expect("summary");
        assert!(summary.contains("model=fake-1"));
        assert!(
            summary.contains("secret=none"),
            "no resident env leaks: {summary}"
        );
        assert_eq!(result["read_only_violation"], false);
        let record = shared
            .task_plane
            .as_ref()
            .expect("plane")
            .load_result(&id)
            .expect("stored");
        assert_eq!(record["kind"], "external_task_result");
        assert_eq!(record["trust"], "external_content");
        assert_eq!(
            record["cost"]["reported_usd"],
            Value::Null,
            "unknown cost stays unknown"
        );
        assert!(record["provenance"]["started_at"].is_string());
    }

    #[test]
    fn metered_and_write_requests_are_refused() {
        let (shared, _dir) = fixture(false);
        let (_, reply) = tool(
            &shared,
            "task_delegate",
            &json!({"objective": "x", "model": "fake:fake-metered"}),
        );
        assert_eq!(reply["ok"], false);
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or("")
                .contains("allow_metered")
        );
        let (_, reply) = tool(
            &shared,
            "task_delegate",
            &json!({"objective": "x", "permissions": "workspace_write"}),
        );
        assert_eq!(reply["ok"], false);
        let (_, reply) = tool(
            &shared,
            "task_delegate",
            &json!({"objective": "x", "workspace": "/etc"}),
        );
        assert_eq!(reply["ok"], false, "only allowlisted workspaces");
    }

    #[test]
    fn running_tasks_can_be_cancelled() {
        let (shared, _dir) = fixture(false);
        let (_, reply) = tool(&shared, "task_delegate", &json!({"objective": "SLEEP"}));
        let id = reply["result"]["task_id"].as_str().expect("id").to_owned();
        std::thread::sleep(Duration::from_millis(300));
        let (_, cancelled) = tool(&shared, "task_cancel", &json!({"id": id}));
        assert_eq!(cancelled["ok"], true, "{cancelled}");
        let task = wait_finished(&shared, &id);
        assert_eq!(task.status, TaskStatus::Cancelled);
        let (_, again) = tool(&shared, "task_cancel", &json!({"id": id}));
        assert_eq!(again["ok"], false);
    }

    #[test]
    fn a_read_only_task_that_writes_fails() {
        let (shared, _dir) = fixture(false);
        let (_, reply) = tool(&shared, "task_delegate", &json!({"objective": "WRITE"}));
        let id = reply["result"]["task_id"].as_str().expect("id").to_owned();
        let task = wait_finished(&shared, &id);
        assert_eq!(task.status, TaskStatus::Failed);
        let (_, status) = tool(&shared, "task_status", &json!({"id": id}));
        assert_eq!(status["result"]["result"]["read_only_violation"], true);
        assert_eq!(
            status["result"]["result"]["files_changed"],
            json!(["written.txt"])
        );
    }

    #[test]
    fn executors_file_changes_apply_without_restart() {
        let (shared, dir) = fixture(true);
        let plane = shared.task_plane.as_ref().expect("plane");
        assert_eq!(plane.registry().names(), vec!["fake"]);
        let file = dir.path().join("executors.json");
        // Remove every executor.
        std::fs::write(&file, r#"{"executors": []}"#).expect("write");
        assert!(plane.registry().is_empty());
        // A broken file keeps the last good list and says why.
        std::fs::write(&file, "{").expect("write");
        assert!(plane.registry().is_empty());
        assert!(plane.status()["config_error"].is_string());
        // Adding one back needs no code and no restart.
        std::fs::write(
            &file,
            json!({"executors": [{"name": "oc", "adapter": "opencode", "command": "/bin/echo"}]})
                .to_string(),
        )
        .expect("write");
        assert_eq!(plane.registry().names(), vec!["oc"]);
        assert!(plane.status()["config_error"].is_null());
    }
}
