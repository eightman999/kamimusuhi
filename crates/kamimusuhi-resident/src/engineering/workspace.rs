//! Workspace operations for the resident's native engineering loop.
//!
//! File actions refuse traversal, symlinks and known credential paths.
//! `Run` uses an exact program allowlist and a minimal environment; this
//! is not an OS sandbox. An allowed program can access other paths or the
//! network, so the operator must choose commands appropriate to the job.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use kamimusuhi_runtime::agent_exec::process::{self, ProcessEnd, ProcessResult, ProcessSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_PATH_BYTES: usize = 1024;
const MAX_CONTENT_BYTES: usize = 1024 * 1024;
const MAX_RANGE_LINES: usize = 200;
const MAX_RANGE_OUTPUT_BYTES: usize = 6 * 1024;
const MAX_REPLACE_BYTES: usize = 16 * 1024;
const MAX_ARG_BYTES: usize = 16 * 1024;
const MAX_ARGS: usize = 64;
const MAX_CHECKS: usize = 16;
const MAX_ENTRIES: usize = 256;
const MAX_CHANGED_BYTES: u64 = 8 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    pub name: String,
    pub path: PathBuf,
    pub checks: Vec<Vec<String>>,
    #[serde(default)]
    pub commands: Vec<String>,
}

impl WorkspaceConfig {
    pub fn validate(&self) -> Result<(), String> {
        identifier(&self.name)?;
        let path = self.path.to_str().ok_or("workspace path must be UTF-8")?;
        if !self.path.is_absolute()
            || path.is_empty()
            || path.len() > 4096
            || path.chars().any(char::is_control)
        {
            return Err("invalid workspace path".to_owned());
        }
        if self.checks.is_empty() || self.checks.len() > MAX_CHECKS {
            return Err("workspace requires 1..16 operator validation commands".to_owned());
        }
        for argv in &self.checks {
            validate_argv(argv)?;
        }
        if self.commands.len() > MAX_ARGS {
            return Err("too many allowed programs".to_owned());
        }
        for program in &self.commands {
            validate_program(program)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub path: PathBuf,
    pub branch: String,
    pub base_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    List {
        path: String,
    },
    Read {
        path: String,
    },
    ReadRange {
        path: String,
        start_line: usize,
        max_lines: usize,
    },
    Write {
        path: String,
        content: String,
        #[serde(default)]
        expected: Option<String>,
    },
    Replace {
        path: String,
        old: String,
        new: String,
    },
    Run {
        argv: Vec<String>,
    },
    Diff,
    Verify,
    Checkpoint {
        summary: String,
        next_step: String,
    },
    Finish {
        summary: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub ok: bool,
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Validation {
    pub passed: bool,
    pub head: String,
    pub checks: Vec<Value>,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub operation_timeout: Duration,
    pub validation_timeout: Duration,
    pub output_bytes: usize,
}

fn identifier(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("identifier must contain 1..80 ASCII letters, digits, '-' or '_'".to_owned());
    }
    Ok(())
}

fn validate_program(program: &str) -> Result<(), String> {
    if program.is_empty()
        || program.len() > MAX_PATH_BYTES
        || program != program.trim()
        || program.starts_with('-')
        || program.chars().any(char::is_control)
    {
        return Err("invalid program".to_owned());
    }
    Ok(())
}

fn validate_argv(argv: &[String]) -> Result<(), String> {
    let Some(program) = argv.first() else {
        return Err("command requires a program".to_owned());
    };
    validate_program(program)?;
    if argv.len() > MAX_ARGS
        || argv.iter().any(|s| s.contains('\0'))
        || argv.iter().map(String::len).sum::<usize>() > MAX_ARG_BYTES
    {
        return Err("command exceeds argument limits".to_owned());
    }
    Ok(())
}

fn limits_valid(limits: &Limits) -> Result<(), String> {
    if limits.operation_timeout.is_zero()
        || limits.validation_timeout.is_zero()
        || !(512..=MAX_CONTENT_BYTES).contains(&limits.output_bytes)
    {
        return Err("timeouts must be positive and output_bytes must be 512..1048576".to_owned());
    }
    Ok(())
}

fn run(
    directory: &Path,
    argv: &[String],
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<ProcessResult, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_owned());
    }
    if timeout.is_zero() {
        return Err("deadline exceeded".to_owned());
    }
    let (program, args) = argv.split_first().ok_or("empty command")?;
    process::run(
        &ProcessSpec {
            program,
            args,
            cwd: Some(directory),
            env_passthrough: &[],
            extra_env: &[],
            timeout,
        },
        cancel,
        |_| {},
    )
}

fn git_with(
    directory: &Path,
    args: &[&str],
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let mut argv = vec![
        "git".to_owned(),
        "-c".to_owned(),
        "core.hooksPath=/dev/null".to_owned(),
        "-c".to_owned(),
        "core.fsmonitor=false".to_owned(),
    ];
    argv.extend(args.iter().map(|s| (*s).to_owned()));
    let outcome = run(directory, &argv, timeout, cancel)?;
    if !outcome.success() {
        return Err(format!(
            "git {} failed (exit {:?}, {:?})",
            args.first().unwrap_or(&"inspection"),
            outcome.exit_code,
            outcome.end
        ));
    }
    if outcome.stdout_truncated || outcome.stdout.len() > MAX_CONTENT_BYTES {
        return Err("git inspection output is incomplete or exceeds limit".to_owned());
    }
    Ok(outcome.stdout.trim_end_matches(['\r', '\n']).to_owned())
}

fn git(directory: &Path, args: &[&str]) -> Result<String, String> {
    git_with(directory, args, GIT_TIMEOUT, &AtomicBool::new(false))
}

pub(super) fn future_path(path: &Path) -> Result<PathBuf, String> {
    let mut missing = Vec::new();
    let mut existing = path.to_path_buf();
    while !existing.exists() {
        missing.push(
            existing
                .file_name()
                .ok_or("invalid candidate root")?
                .to_owned(),
        );
        existing = existing
            .parent()
            .ok_or("invalid candidate root")?
            .to_path_buf();
        if existing.as_os_str().is_empty() {
            existing = PathBuf::from(".");
        }
    }
    let mut resolved = existing
        .canonicalize()
        .map_err(|_| "cannot resolve candidate root")?;
    for part in missing.iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

/// Plan without creating anything: persist this value before `prepare`.
pub fn plan(policy: &WorkspaceConfig, root: &Path, id: &str) -> Result<Candidate, String> {
    policy.validate()?;
    identifier(id)?;
    let workspace = policy
        .path
        .canonicalize()
        .map_err(|_| "workspace is unavailable")?;
    let top = git(&workspace, &["rev-parse", "--show-toplevel"])?;
    if Path::new(&top).canonicalize().ok().as_ref() != Some(&workspace) {
        return Err("workspace must be the repository root".to_owned());
    }
    let path = future_path(&root.join(&policy.name).join(id))?;
    if path.starts_with(&workspace) || workspace.starts_with(&path) {
        return Err("candidate must be outside the original workspace".to_owned());
    }
    Ok(Candidate {
        path,
        branch: format!("kamimusuhi/engineering/{id}"),
        base_commit: git(&workspace, &["rev-parse", "--verify", "HEAD"])?,
    })
}

fn candidate_valid(policy: &WorkspaceConfig, candidate: &Candidate) -> Result<(), String> {
    policy.validate()?;
    let id = candidate
        .branch
        .strip_prefix("kamimusuhi/engineering/")
        .ok_or("invalid candidate branch")?;
    identifier(id)?;
    if candidate.path.file_name().and_then(|s| s.to_str()) != Some(id)
        || !candidate.path.is_absolute()
        || candidate
            .path
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        || !matches!(candidate.base_commit.len(), 40 | 64)
        || !candidate.base_commit.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("invalid candidate identity".to_owned());
    }
    let workspace = policy
        .path
        .canonicalize()
        .map_err(|_| "workspace is unavailable")?;
    let path = future_path(&candidate.path)?;
    if path != candidate.path || path.starts_with(&workspace) || workspace.starts_with(&path) {
        return Err("candidate path is not isolated".to_owned());
    }
    Ok(())
}

/// Resume preparation only for this registered branch/path/base. Existing
/// candidate edits are preserved, including edits not yet committed.
pub fn prepare(policy: &WorkspaceConfig, candidate: &Candidate) -> Result<(), String> {
    candidate_valid(policy, candidate)?;
    let registration = git(&policy.path, &["worktree", "list", "--porcelain"])?;
    let path_text = candidate
        .path
        .to_str()
        .ok_or("candidate path must be UTF-8")?;
    let branch_ref = format!("refs/heads/{}", candidate.branch);
    let mut registered = false;
    for block in registration.split("\n\n") {
        let path = block
            .lines()
            .find_map(|line| line.strip_prefix("worktree "));
        let branch = block.lines().find_map(|line| line.strip_prefix("branch "));
        if path == Some(path_text) {
            if branch != Some(branch_ref.as_str()) {
                return Err("candidate path is registered to a different branch".to_owned());
            }
            registered = true;
        } else if branch == Some(branch_ref.as_str()) {
            return Err("candidate branch belongs to another worktree".to_owned());
        }
    }
    if candidate.path.exists() {
        if !registered || !candidate.path.is_dir() {
            return Err("candidate path already exists without matching registration".to_owned());
        }
        let top = git(&candidate.path, &["rev-parse", "--show-toplevel"])?;
        if Path::new(&top).canonicalize().ok().as_ref() != Some(&candidate.path)
            || git(&candidate.path, &["symbolic-ref", "--short", "HEAD"])? != candidate.branch
        {
            return Err("candidate worktree identity changed".to_owned());
        }
        git(
            &candidate.path,
            &[
                "merge-base",
                "--is-ancestor",
                &candidate.base_commit,
                "HEAD",
            ],
        )?;
        recover_write(candidate)?;
        return Ok(());
    }
    if registered {
        return Err("candidate registration exists but its directory is missing".to_owned());
    }
    let parent = candidate.path.parent().ok_or("candidate has no parent")?;
    fs::create_dir_all(parent).map_err(|_| "cannot create candidate directory")?;
    git(
        &policy.path,
        &[
            "cat-file",
            "-e",
            &format!("{}^{{commit}}", candidate.base_commit),
        ],
    )?;
    let existing_branch = git(
        &policy.path,
        &["show-ref", "--verify", "--hash", &branch_ref],
    );
    if let Ok(head) = existing_branch {
        if head != candidate.base_commit {
            return Err("unregistered candidate branch has changed".to_owned());
        }
        git(
            &policy.path,
            &["worktree", "add", "-q", path_text, &candidate.branch],
        )?;
    } else {
        git(
            &policy.path,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &candidate.branch,
                path_text,
                &candidate.base_commit,
            ],
        )?;
    }
    Ok(())
}

fn forbidden_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        ".git"
            | ".env"
            | ".ssh"
            | ".aws"
            | ".gnupg"
            | ".netrc"
            | ".npmrc"
            | ".pypirc"
            | "credentials"
            | "credentials.json"
            | "auth.json"
            | "secrets"
            | "id_rsa"
            | "id_ed25519"
            | "id_ecdsa"
            | "id_dsa"
    ) || name.starts_with(".env.")
        || name.starts_with("secrets.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".p12")
        || name.ends_with(".pfx")
        || name.starts_with(".kamimusuhi-write-")
}

fn relative_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.contains('\\')
        || path.chars().any(char::is_control)
    {
        return Err("invalid relative path".to_owned());
    }
    let mut relative = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => {
                let name = name.to_str().ok_or("path must be UTF-8")?;
                if forbidden_name(name) {
                    return Err("credential and git metadata paths are not accessible".to_owned());
                }
                relative.push(name);
            }
            _ => return Err("path must stay inside the candidate".to_owned()),
        }
    }
    Ok(relative)
}

fn safe_path(candidate: &Candidate, path: &str) -> Result<PathBuf, String> {
    let relative = relative_path(path)?;
    let root_meta =
        fs::symlink_metadata(&candidate.path).map_err(|_| "candidate is unavailable")?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Err("candidate root must be a real directory".to_owned());
    }
    let mut current = candidate.path.clone();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("symlink paths are not accessible".to_owned());
            }
            Ok(meta) => {
                if !meta.is_file() && !meta.is_dir() {
                    return Err("special filesystem objects are not accessible".to_owned());
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if meta.is_file() && meta.nlink() > 1 {
                        return Err("hard-linked files are not accessible".to_owned());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("cannot inspect path".to_owned()),
        }
    }
    Ok(current)
}

fn read_file(path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err("path is not a regular file".to_owned());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "cannot open file")?
        .take((MAX_CONTENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read file")?;
    if bytes.len() > MAX_CONTENT_BYTES {
        return Err("file exceeds 1 MiB input limit".to_owned());
    }
    String::from_utf8(bytes).map_err(|_| "file is not UTF-8 text".to_owned())
}

fn read_range(
    path: &str,
    content: &str,
    start_line: usize,
    max_lines: usize,
    output_bytes: usize,
) -> Result<Value, String> {
    if start_line == 0 || !(1..=MAX_RANGE_LINES).contains(&max_lines) {
        return Err("read_range requires start_line >= 1 and max_lines = 1..200".to_owned());
    }
    // Keep line endings, including CRLF, so a returned snippet can be used
    // as an exact Replace precondition. A final LF creates no phantom line.
    let total_lines = content.split_inclusive('\n').count();
    let view = |text: &str, end_line: Option<usize>, next_line: Option<usize>| {
        json!({
            "path":path, "start_line":start_line, "end_line":end_line,
            "next_line":next_line, "total_lines":total_lines,
            "truncated":next_line.is_some(), "content":text,
        })
    };
    let fits =
        |detail: &Value| serde_json::to_vec(detail).is_ok_and(|bytes| bytes.len() <= output_bytes);
    let mut detail = view("", None, (start_line <= total_lines).then_some(start_line));
    if !fits(&detail) {
        return Err("read_range metadata exceeds the output budget".to_owned());
    }
    let mut selected = String::new();
    for (index, line) in content
        .split_inclusive('\n')
        .enumerate()
        .skip(start_line - 1)
        .take(max_lines)
    {
        selected.push_str(line);
        let line_number = index + 1;
        let next_line = (line_number < total_lines).then_some(line_number + 1);
        let proposed = view(&selected, Some(line_number), next_line);
        if !fits(&proposed) {
            if detail["end_line"].is_null() {
                return Err(format!(
                    "line {line_number} exceeds the read_range output budget; no partial line was returned"
                ));
            }
            // Keep the previous view: its next_line points at this entire
            // unreturned line. Generic string truncation would skip data.
            break;
        }
        detail = proposed;
    }
    Ok(detail)
}

fn replacement(original: &str, old: &str, new: &str) -> Result<(String, usize), String> {
    let first_character = old.chars().next().ok_or("replace old must not be empty")?;
    if old == new {
        return Err("replace must change the matched text".to_owned());
    }
    if old.len() > MAX_REPLACE_BYTES || new.len() > MAX_REPLACE_BYTES {
        return Err("replace old and new must each be at most 16 KiB".to_owned());
    }
    let first = original
        .find(old)
        .ok_or("replace conflict: old text was not found")?;
    // match_indices()/matches() count non-overlapping hits only: aa in aaa
    // is still ambiguous. Advance one UTF-8 character to detect overlaps.
    if original[first + first_character.len_utf8()..].contains(old) {
        return Err("replace conflict: old text occurs more than once".to_owned());
    }
    let bytes = original
        .len()
        .checked_sub(old.len())
        .and_then(|n| n.checked_add(new.len()))
        .filter(|n| *n <= MAX_CONTENT_BYTES)
        .ok_or("replacement exceeds the 1 MiB file limit")?;
    let mut updated = String::with_capacity(bytes);
    updated.push_str(&original[..first]);
    updated.push_str(new);
    updated.push_str(&original[first + old.len()..]);
    let start_line = original[..first].bytes().filter(|b| *b == b'\n').count() + 1;
    Ok((updated, start_line))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteIntent {
    branch: String,
    base_commit: String,
    path: String,
    nonce: String,
}

fn write_journal(candidate: &Candidate) -> Result<PathBuf, String> {
    let id = candidate
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("invalid candidate path")?;
    identifier(id)?;
    Ok(candidate.path.with_file_name(format!(".{id}.write.json")))
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "cannot sync directory".to_owned())
}

/// A crash can leave a complete new file and its temporary hard link, or
/// an unused partial temp. Only a durable intent proves which temp we own.
fn recover_write(candidate: &Candidate) -> Result<(), String> {
    let journal = write_journal(candidate)?;
    match fs::symlink_metadata(&journal) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 4096 => {}
        _ => return Err("invalid write recovery journal".to_owned()),
    }
    let bytes = fs::read(&journal).map_err(|_| "cannot read write recovery journal")?;
    let intent: WriteIntent =
        serde_json::from_slice(&bytes).map_err(|_| "invalid write recovery journal")?;
    if intent.branch != candidate.branch
        || intent.base_commit != candidate.base_commit
        || intent.nonce.len() != 32
        || !intent.nonce.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("write recovery journal does not belong to this candidate".to_owned());
    }
    let relative = relative_path(&intent.path)?;
    relative
        .file_name()
        .ok_or("write journal names a directory")?;
    let parent = relative.parent().filter(|p| !p.as_os_str().is_empty());
    let parent = safe_path(candidate, parent.and_then(Path::to_str).unwrap_or("."))?;
    let temporary = parent.join(format!(".kamimusuhi-write-{}", intent.nonce));
    match fs::symlink_metadata(&temporary) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
            fs::remove_file(&temporary).map_err(|_| "cannot remove owned temporary file")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err("write recovery temporary path changed".to_owned()),
    }
    sync_directory(&parent)?;
    fs::remove_file(&journal).map_err(|_| "cannot clear write recovery journal")?;
    sync_directory(journal.parent().ok_or("write journal has no parent")?)
}

struct OwnedTemp {
    path: PathBuf,
    journal: PathBuf,
}

impl Drop for OwnedTemp {
    fn drop(&mut self) {
        // The path is generated by this invocation and acquired with
        // create_new. Never remove another writer's existing file.
        let removed = match fs::remove_file(&self.path) {
            Ok(()) => true,
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        };
        if removed {
            let _ = fs::remove_file(&self.journal);
        }
    }
}

fn atomic_write(
    candidate: &Candidate,
    path: &str,
    content: &str,
    expected: Option<&str>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let destination = safe_path(candidate, path)?;
    let check_expected = |destination: &Path| -> Result<(), String> {
        match expected {
            Some(expected) if read_file(destination)? != expected => {
                Err("write conflict: current content differs from expected".to_owned())
            }
            None if destination.exists() => {
                Err("write conflict: new file already exists".to_owned())
            }
            _ => Ok(()),
        }
    };
    check_expected(&destination)?;
    let permissions = expected
        .map(|_| fs::metadata(&destination).map(|meta| meta.permissions()))
        .transpose()
        .map_err(|_| "cannot inspect destination permissions")?;
    let parent = destination.parent().ok_or("missing parent")?;
    fs::create_dir_all(parent).map_err(|_| "cannot create parent directory")?;
    safe_path(candidate, path)?;
    let nonce = process::random_hex().ok_or("cannot allocate temporary file name")?;
    let temporary = parent.join(format!(".kamimusuhi-write-{nonce}"));
    let journal = write_journal(candidate)?;
    let intent = WriteIntent {
        branch: candidate.branch.clone(),
        base_commit: candidate.base_commit.clone(),
        path: path.to_owned(),
        nonce,
    };
    crate::util::atomic_write(
        &journal,
        &serde_json::to_vec(&intent).map_err(|_| "cannot encode write intent")?,
    )
    .map_err(|_| "cannot persist write intent")?;
    sync_directory(journal.parent().ok_or("write journal has no parent")?)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(&temporary) {
        Ok(file) => file,
        Err(_) => {
            // No temp was acquired, so remove only the intent we wrote.
            let _ = fs::remove_file(&journal);
            return Err("cannot create temporary file".to_owned());
        }
    };
    let owned = OwnedTemp {
        path: temporary,
        journal,
    };
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)
            .map_err(|_| "cannot preserve destination permissions")?;
    }
    file.write_all(content.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|_| "cannot persist temporary file")?;
    drop(file);
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_owned());
    }
    let destination = safe_path(candidate, path)?;
    check_expected(&destination)?;
    if expected.is_some() {
        fs::rename(&owned.path, &destination).map_err(|_| "cannot replace destination")?;
    } else {
        // A rename would overwrite a file created since the CAS check.
        // link is atomic and refuses an existing destination.
        fs::hard_link(&owned.path, &destination)
            .map_err(|_| "write conflict: destination could not be created")?;
    }
    drop(owned);
    sync_directory(parent)?;
    Ok(())
}

fn clip(text: &str, bytes: usize) -> String {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Bound JSON as encoded, including escaping of diagnostic output.
fn bounded(mut value: Value, bytes: usize) -> Value {
    loop {
        if serde_json::to_vec(&value).is_ok_and(|b| b.len() <= bytes) {
            return value;
        }
        value["truncated"] = json!(true);
        let longest = value
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(key, value)| {
                value
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| (key.clone(), s.len()))
            })
            .max_by_key(|(_, len)| *len);
        if let Some((key, len)) = longest {
            value[&key] = json!(clip(value[&key].as_str().unwrap_or_default(), len / 2));
        } else if let Some(entries) = value.get_mut("entries").and_then(Value::as_array_mut)
            && !entries.is_empty()
        {
            entries.truncate(entries.len() / 2);
        } else {
            return json!({"truncated":true});
        }
    }
}

fn process_detail(index: usize, program: &str, result: Result<ProcessResult, String>) -> Value {
    match result {
        Ok(outcome) => json!({
            "index": index, "program": program, "passed": outcome.success(),
            "exit_code": outcome.exit_code, "duration_ms": outcome.duration_ms,
            "end": match outcome.end { ProcessEnd::Exited => "exited", ProcessEnd::TimedOut => "timed_out", ProcessEnd::Cancelled => "cancelled" },
            "stdout": outcome.stdout, "stderr": outcome.stderr_tail,
            "stdout_truncated": outcome.stdout_truncated, "stderr_truncated": outcome.stderr_truncated,
        }),
        Err(error) => {
            json!({"index":index,"program":program,"passed":false,"exit_code":null,"error":error})
        }
    }
}

pub fn execute(
    policy: &WorkspaceConfig,
    candidate: &Candidate,
    action: &Action,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<Observation, String> {
    limits_valid(limits)?;
    candidate_valid(policy, candidate)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_owned());
    }
    recover_write(candidate)?;
    let (ok, detail) = match action {
        Action::List { path } => {
            let path = if path.is_empty() { "." } else { path };
            let directory = safe_path(candidate, path)?;
            let mut entries = Vec::new();
            let mut truncated = false;
            for (index, entry) in fs::read_dir(directory)
                .map_err(|_| "cannot list directory")?
                .take(MAX_ENTRIES + 1)
                .enumerate()
            {
                if index == MAX_ENTRIES {
                    truncated = true;
                    break;
                }
                let entry = entry.map_err(|_| "cannot read directory entry")?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "file name is not UTF-8")?;
                if forbidden_name(&name) {
                    continue;
                }
                let kind = entry.file_type().map_err(|_| "cannot inspect entry")?;
                entries.push(json!({"name":name,"kind":if kind.is_symlink() {"symlink"} else if kind.is_dir() {"directory"} else if kind.is_file() {"file"} else {"special"}}));
            }
            entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            (
                true,
                json!({"path":path,"entries":entries,"truncated":truncated}),
            )
        }
        Action::Read { path } => {
            let content = read_file(&safe_path(candidate, path)?)?;
            (
                true,
                json!({"path":path,"bytes":content.len(),"content":content}),
            )
        }
        Action::ReadRange {
            path,
            start_line,
            max_lines,
        } => {
            let content = read_file(&safe_path(candidate, path)?)?;
            let output_bytes = limits
                .output_bytes
                .saturating_sub(32)
                .min(MAX_RANGE_OUTPUT_BYTES);
            let detail = read_range(path, &content, *start_line, *max_lines, output_bytes)?;
            // The complete view already fits both operation and context
            // budgets. Never cut content independently of its cursor.
            return Ok(Observation { ok: true, detail });
        }
        Action::Write {
            path,
            content,
            expected,
        } => {
            if content.len() > MAX_CONTENT_BYTES
                || expected
                    .as_ref()
                    .is_some_and(|s| s.len() > MAX_CONTENT_BYTES)
            {
                return Err("write exceeds 1 MiB input limit".to_owned());
            }
            atomic_write(candidate, path, content, expected.as_deref(), cancel)?;
            (true, json!({"path":path,"written_bytes":content.len()}))
        }
        Action::Replace { path, old, new } => {
            let original = read_file(&safe_path(candidate, path)?)?;
            let (updated, start_line) = replacement(&original, old, new)?;
            atomic_write(candidate, path, &updated, Some(&original), cancel)?;
            (
                true,
                json!({"path":path,"replacements":1,"start_line":start_line,
                "before_bytes":original.len(),"after_bytes":updated.len(),
                "removed_bytes":old.len(),"inserted_bytes":new.len()}),
            )
        }
        Action::Run { argv } => {
            validate_argv(argv)?;
            if !policy.commands.contains(&argv[0]) {
                return Err("program is not in the workspace command allowlist".to_owned());
            }
            let result = run(&candidate.path, argv, limits.operation_timeout, cancel);
            let ok = result.as_ref().is_ok_and(ProcessResult::success);
            (ok, process_detail(0, &argv[0], result))
        }
        Action::Diff => {
            let names = git_with(
                &candidate.path,
                &[
                    "diff",
                    "--name-only",
                    "--no-renames",
                    &candidate.base_commit,
                    "--",
                ],
                limits.operation_timeout,
                cancel,
            )?;
            for name in names.lines() {
                if name.starts_with('"') {
                    return Err("quoted git paths are not supported".to_owned());
                }
                safe_path(candidate, name)?;
            }
            let diff = git_with(
                &candidate.path,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-color",
                    &candidate.base_commit,
                    "--",
                ],
                limits.operation_timeout,
                cancel,
            )?;
            (true, json!({"diff":diff}))
        }
        Action::Verify | Action::Checkpoint { .. } | Action::Finish { .. } => {
            return Err("lifecycle actions must be handled by the engineering loop".to_owned());
        }
    };
    Ok(Observation {
        ok,
        detail: bounded(detail, limits.output_bytes.saturating_sub(32)),
    })
}

pub fn revision(candidate: &Candidate) -> Result<String, String> {
    git(&candidate.path, &["rev-parse", "--verify", "HEAD"])
}

pub fn clean(candidate: &Candidate) -> Result<bool, String> {
    recover_write(candidate)?;
    Ok(git(
        &candidate.path,
        &["status", "--porcelain", "--untracked-files=all"],
    )?
    .is_empty())
}

pub fn verify(
    policy: &WorkspaceConfig,
    candidate: &Candidate,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<Validation, String> {
    limits_valid(limits)?;
    candidate_valid(policy, candidate)?;
    recover_write(candidate)?;
    let check_bytes = limits.output_bytes.saturating_sub(256) / policy.checks.len();
    if check_bytes < 512 {
        return Err("output budget is too small for validation records".to_owned());
    }
    let deadline = Instant::now() + limits.validation_timeout;
    let git = |args: &[&str]| {
        git_with(
            &candidate.path,
            args,
            deadline.saturating_duration_since(Instant::now()),
            cancel,
        )
    };
    if git(&["symbolic-ref", "--short", "HEAD"])? != candidate.branch {
        return Err("candidate branch changed".to_owned());
    }
    git(&[
        "merge-base",
        "--is-ancestor",
        &candidate.base_commit,
        "HEAD",
    ])?;
    // Refuse committing credential paths and special files, including
    // changes produced by an operator-allowed Run command.
    let mut changed = Vec::new();
    for args in [
        vec![
            "diff",
            "--name-only",
            "--no-renames",
            &candidate.base_commit,
            "--",
        ],
        vec!["ls-files", "--others", "--exclude-standard"],
    ] {
        let names = git(&args)?;
        if names.len() > MAX_PATH_BYTES * MAX_ENTRIES {
            return Err("too many changed paths".to_owned());
        }
        for name in names.lines() {
            if name.starts_with('"') {
                return Err("quoted git paths are not supported by validation".to_owned());
            }
            changed.push(name.to_owned());
        }
    }
    changed.sort();
    changed.dedup();
    if changed.len() > MAX_ENTRIES {
        return Err("too many changed files".to_owned());
    }
    let mut bytes = 0u64;
    for name in changed {
        let path = safe_path(candidate, &name)?;
        if let Ok(meta) = fs::metadata(path) {
            bytes = bytes.saturating_add(meta.len());
            if bytes > MAX_CHANGED_BYTES {
                return Err("changed files exceed capture limit".to_owned());
            }
        }
    }
    git(&["add", "-A"])?;
    if !git(&["diff", "--cached", "--name-only"])?.is_empty() {
        git(&[
            "-c",
            "user.name=Kamimusuhi Engineering",
            "-c",
            "user.email=engineering@kamimusuhi.local",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--no-verify",
            "-m",
            "fix(engineering): capture candidate for validation",
        ])?;
    }
    let head = git(&["rev-parse", "--verify", "HEAD"])?;
    let mut checks = Vec::new();
    let mut passed = true;
    for (index, argv) in policy.checks.iter().enumerate() {
        let record = process_detail(
            index,
            &argv[0],
            run(
                &candidate.path,
                argv,
                deadline.saturating_duration_since(Instant::now()),
                cancel,
            ),
        );
        passed &= record["passed"] == true;
        checks.push(bounded(record, check_bytes));
        if !passed {
            break;
        }
    }
    let unchanged = git(&["rev-parse", "--verify", "HEAD"]).is_ok_and(|now| now == head)
        && git(&["symbolic-ref", "--short", "HEAD"]).is_ok_and(|now| now == candidate.branch)
        && git(&["status", "--porcelain", "--untracked-files=all"])
            .is_ok_and(|status| status.is_empty());
    if !unchanged {
        passed = false;
        checks.push(json!({"phase":"integrity","passed":false,"error":"candidate changed or could not be inspected after validation"}));
    }
    Ok(Validation {
        passed,
        head,
        checks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, WorkspaceConfig, Candidate, Limits) {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("source");
        fs::create_dir(&source).expect("source");
        git(&source, &["init", "-q"]).expect("git init");
        fs::write(source.join("code.txt"), "before\n").expect("code");
        git(&source, &["add", "code.txt"]).expect("add");
        commit(&source);
        fs::write(source.join("operator-draft.txt"), "keep this draft").expect("draft");
        let policy = WorkspaceConfig {
            name: "fixture".to_owned(),
            path: source,
            checks: vec![vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "test \"$(cat code.txt)\" = fixed".to_owned(),
            ]],
            commands: vec!["sh".to_owned(), "env".to_owned()],
        };
        let candidate = plan(&policy, &dir.path().join("worktrees"), "job-1").expect("plan");
        let limits = Limits {
            operation_timeout: Duration::from_secs(5),
            validation_timeout: Duration::from_secs(10),
            output_bytes: 8192,
        };
        (dir, policy, candidate, limits)
    }

    fn commit(path: &Path) {
        git(
            path,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@local",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "fixture",
            ],
        )
        .expect("commit");
    }

    fn perform(
        policy: &WorkspaceConfig,
        candidate: &Candidate,
        limits: &Limits,
        action: Action,
    ) -> Result<Observation, String> {
        execute(policy, candidate, &action, limits, &AtomicBool::new(false))
    }

    #[test]
    fn ranges_preserve_utf8_line_endings_and_eof_without_phantom_lines() {
        let (_dir, policy, candidate, limits) = fixture();
        prepare(&policy, &candidate).unwrap();
        for (content, total_lines) in [
            ("", 0),
            ("一行\n", 1),
            ("first\r\n\n最後🙂", 3),
            ("a\n\n", 2),
        ] {
            fs::write(candidate.path.join("range.txt"), content).unwrap();
            let mut cursor = 1;
            let mut collected = String::new();
            loop {
                let view = perform(
                    &policy,
                    &candidate,
                    &limits,
                    Action::ReadRange {
                        path: "range.txt".to_owned(),
                        start_line: cursor,
                        max_lines: 2,
                    },
                )
                .unwrap()
                .detail;
                assert_eq!(view["start_line"], cursor);
                assert_eq!(view["total_lines"], total_lines);
                collected.push_str(view["content"].as_str().unwrap());
                let Some(next) = view["next_line"].as_u64() else {
                    assert_eq!(view["truncated"], false);
                    break;
                };
                assert_eq!(view["truncated"], true);
                assert!(next as usize > cursor);
                assert_eq!(view["end_line"].as_u64(), Some(next - 1));
                cursor = next as usize;
                assert!(cursor <= total_lines);
            }
            assert_eq!(collected, content);
            let eof = perform(
                &policy,
                &candidate,
                &limits,
                Action::ReadRange {
                    path: "range.txt".to_owned(),
                    start_line: total_lines + 1,
                    max_lines: 1,
                },
            )
            .unwrap()
            .detail;
            assert_eq!(eof["content"], "");
            assert_eq!(eof["end_line"], Value::Null);
            assert_eq!(eof["next_line"], Value::Null);
            assert_eq!(eof["truncated"], false);
        }
        for (start_line, max_lines) in [(0, 1), (1, 0), (1, MAX_RANGE_LINES + 1)] {
            assert!(
                perform(
                    &policy,
                    &candidate,
                    &limits,
                    Action::ReadRange {
                        path: "range.txt".to_owned(),
                        start_line,
                        max_lines
                    }
                )
                .is_err()
            );
        }
    }

    #[test]
    fn ranges_fit_encoded_budgets_without_skipping_a_whole_or_oversized_line() {
        let (_dir, policy, candidate, mut limits) = fixture();
        prepare(&policy, &candidate).unwrap();
        let content: String = (0..150)
            .map(|i| format!("行{i:03}: \"\\{}\"\r\n", "é🙂".repeat(18)))
            .collect();
        fs::write(candidate.path.join("range.txt"), &content).unwrap();
        limits.output_bytes = 32_768;
        let first = perform(
            &policy,
            &candidate,
            &limits,
            Action::ReadRange {
                path: "range.txt".to_owned(),
                start_line: 1,
                max_lines: 200,
            },
        )
        .unwrap();
        assert!(serde_json::to_vec(&first.detail).unwrap().len() <= MAX_RANGE_OUTPUT_BYTES);
        assert_eq!(first.detail["truncated"], true);
        limits.output_bytes = 512;
        let mut cursor = 1;
        let mut collected = String::new();
        let mut pages = 0;
        loop {
            let page = perform(
                &policy,
                &candidate,
                &limits,
                Action::ReadRange {
                    path: "range.txt".to_owned(),
                    start_line: cursor,
                    max_lines: 200,
                },
            )
            .unwrap();
            pages += 1;
            assert!(pages <= 150, "every successful page must advance");
            assert!(serde_json::to_vec(&page).unwrap().len() <= limits.output_bytes);
            assert!(serde_json::to_vec(&page.detail).unwrap().len() <= limits.output_bytes - 32);
            collected.push_str(page.detail["content"].as_str().unwrap());
            let Some(next) = page.detail["next_line"].as_u64() else {
                break;
            };
            assert!(next as usize > cursor);
            cursor = next as usize;
        }
        assert!(pages > 1);
        assert_eq!(collected, content);

        let content = format!("short\n{}", "界".repeat(3000));
        fs::write(candidate.path.join("range.txt"), content).unwrap();
        limits.output_bytes = 32_768;
        let first = perform(
            &policy,
            &candidate,
            &limits,
            Action::ReadRange {
                path: "range.txt".to_owned(),
                start_line: 1,
                max_lines: 200,
            },
        )
        .unwrap();
        assert_eq!(first.detail["content"], "short\n");
        assert_eq!(first.detail["next_line"], 2);
        let error = perform(
            &policy,
            &candidate,
            &limits,
            Action::ReadRange {
                path: "range.txt".to_owned(),
                start_line: 2,
                max_lines: 200,
            },
        )
        .unwrap_err();
        assert!(error.contains("line 2 exceeds"));
    }

    #[test]
    fn replace_requires_a_unique_change_including_overlapping_matches() {
        let (_dir, policy, candidate, limits) = fixture();
        prepare(&policy, &candidate).unwrap();
        for (content, old, new) in [
            ("aaa", "aa", "b"),
            ("あああ", "ああ", "い"),
            ("foo + foo", "foo", "bar"),
            ("original", "", "new"),
            ("original", "missing", "new"),
            ("original", "original", "original"),
        ] {
            fs::write(candidate.path.join("replace.txt"), content).unwrap();
            assert!(
                perform(
                    &policy,
                    &candidate,
                    &limits,
                    Action::Replace {
                        path: "replace.txt".to_owned(),
                        old: old.to_owned(),
                        new: new.to_owned()
                    }
                )
                .is_err()
            );
            assert_eq!(
                fs::read_to_string(candidate.path.join("replace.txt")).unwrap(),
                content
            );
        }
        let original = "先頭\r\n対象🙂\r\n終端";
        fs::write(candidate.path.join("replace.txt"), original).unwrap();
        let changed = perform(
            &policy,
            &candidate,
            &limits,
            Action::Replace {
                path: "replace.txt".to_owned(),
                old: "対象🙂".to_owned(),
                new: "変更é".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(changed.detail["replacements"], 1);
        assert_eq!(changed.detail["start_line"], 2);
        assert_eq!(
            fs::read_to_string(candidate.path.join("replace.txt")).unwrap(),
            "先頭\r\n変更é\r\n終端"
        );
        perform(
            &policy,
            &candidate,
            &limits,
            Action::Replace {
                path: "replace.txt".to_owned(),
                old: "\r\n終端".to_owned(),
                new: String::new(),
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(candidate.path.join("replace.txt")).unwrap(),
            "先頭\r\n変更é"
        );
    }

    #[test]
    fn range_and_replace_enforce_secret_paths_and_size_limits_without_writing() {
        let (_dir, policy, candidate, limits) = fixture();
        prepare(&policy, &candidate).unwrap();
        fs::write(candidate.path.join(".env"), "fixture-only").unwrap();
        for action in [
            Action::ReadRange {
                path: ".env".to_owned(),
                start_line: 1,
                max_lines: 1,
            },
            Action::Replace {
                path: ".env".to_owned(),
                old: "fixture-only".to_owned(),
                new: "changed".to_owned(),
            },
        ] {
            assert!(perform(&policy, &candidate, &limits, action).is_err());
        }
        assert_eq!(
            fs::read_to_string(candidate.path.join(".env")).unwrap(),
            "fixture-only"
        );
        let path = candidate.path.join("bounded.txt");
        let oversized = "x".repeat(MAX_REPLACE_BYTES + 1);
        fs::write(&path, &oversized).unwrap();
        assert!(
            perform(
                &policy,
                &candidate,
                &limits,
                Action::Replace {
                    path: "bounded.txt".to_owned(),
                    old: oversized.clone(),
                    new: "short".to_owned()
                }
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), oversized);
        fs::write(&path, "short").unwrap();
        assert!(
            perform(
                &policy,
                &candidate,
                &limits,
                Action::Replace {
                    path: "bounded.txt".to_owned(),
                    old: "short".to_owned(),
                    new: oversized
                }
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "short");
        let at_limit = format!("ABC{}", "x".repeat(MAX_CONTENT_BYTES - 3));
        fs::write(&path, &at_limit).unwrap();
        assert!(
            perform(
                &policy,
                &candidate,
                &limits,
                Action::Replace {
                    path: "bounded.txt".to_owned(),
                    old: "ABC".to_owned(),
                    new: "ABCD".to_owned()
                }
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), at_limit);
        fs::write(&path, "x".repeat(MAX_CONTENT_BYTES + 1)).unwrap();
        assert!(
            perform(
                &policy,
                &candidate,
                &limits,
                Action::ReadRange {
                    path: "bounded.txt".to_owned(),
                    start_line: 1,
                    max_lines: 1
                }
            )
            .is_err()
        );
    }

    #[test]
    fn ranged_edit_repairs_large_rust_source_with_the_real_compiler() {
        let (dir, mut policy, _unused_plan, mut limits) = fixture();
        assert!(
            process::resolve_binary("rustc").is_some(),
            "real rustc must be on the test PATH"
        );
        let mut original: String = (0..6000)
            .map(|i| format!("// 文脈 {i:04}: unchanged source material\n"))
            .collect();
        original.push_str("pub fn answer() -> u32 { unknown_value }");
        assert!(original.len() > 128 * 1024);
        fs::write(policy.path.join("large.rs"), &original).unwrap();
        git(&policy.path, &["add", "large.rs"]).unwrap();
        commit(&policy.path);
        policy.checks = vec![vec![
            "rustc".to_owned(),
            "--crate-type=lib".to_owned(),
            "--emit=metadata".to_owned(),
            "-o".to_owned(),
            dir.path().join("large.rmeta").to_str().unwrap().to_owned(),
            "large.rs".to_owned(),
        ]];
        let candidate = plan(&policy, &dir.path().join("worktrees"), "large-job").unwrap();
        prepare(&policy, &candidate).unwrap();
        limits.validation_timeout = Duration::from_secs(30);
        let failed = verify(&policy, &candidate, &limits, &AtomicBool::new(false)).unwrap();
        assert!(!failed.passed);
        assert_eq!(
            failed.checks[0]["exit_code"], 1,
            "the real compiler rejects the unresolved identifier"
        );
        assert!(
            failed.checks[0]["stderr"]
                .as_str()
                .unwrap()
                .contains("E0425")
        );
        let range = perform(
            &policy,
            &candidate,
            &limits,
            Action::ReadRange {
                path: "large.rs".to_owned(),
                start_line: 5999,
                max_lines: 10,
            },
        )
        .unwrap();
        assert!(
            range.detail["content"]
                .as_str()
                .unwrap()
                .ends_with("pub fn answer() -> u32 { unknown_value }")
        );
        assert_eq!(range.detail["end_line"], 6001);
        assert_eq!(range.detail["next_line"], Value::Null);
        let changed = perform(
            &policy,
            &candidate,
            &limits,
            Action::Replace {
                path: "large.rs".to_owned(),
                old: "unknown_value".to_owned(),
                new: "42".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(changed.detail["replacements"], 1);
        let passed = verify(&policy, &candidate, &limits, &AtomicBool::new(false)).unwrap();
        assert!(passed.passed, "{:?}", passed.checks);
        assert_eq!(passed.checks[0]["exit_code"], 0);
        assert_ne!(failed.head, passed.head);
        assert_eq!(
            fs::read_to_string(candidate.path.join("large.rs")).unwrap(),
            original.replacen("unknown_value", "42", 1)
        );
        assert_eq!(
            fs::read_to_string(policy.path.join("large.rs")).unwrap(),
            original
        );
        assert_eq!(
            fs::read_to_string(policy.path.join("operator-draft.txt")).unwrap(),
            "keep this draft"
        );
        eprintln!(
            "large Rust fixture: {} bytes, rustc exit {} -> {}, range JSON {} bytes, replacements {}",
            original.len(),
            failed.checks[0]["exit_code"],
            passed.checks[0]["exit_code"],
            serde_json::to_vec(&range.detail).unwrap().len(),
            changed.detail["replacements"]
        );
    }

    #[test]
    fn planned_base_and_uncommitted_candidate_survive_repeated_prepare() {
        let (_dir, policy, candidate, limits) = fixture();
        fs::write(policy.path.join("code.txt"), "new base\n").expect("advance");
        git(&policy.path, &["add", "code.txt"]).expect("add");
        commit(&policy.path);
        assert_ne!(
            git(&policy.path, &["rev-parse", "HEAD"]).unwrap(),
            candidate.base_commit
        );
        prepare(&policy, &candidate).expect("prepare recorded base");
        assert_eq!(revision(&candidate).unwrap(), candidate.base_commit);
        perform(
            &policy,
            &candidate,
            &limits,
            Action::Write {
                path: "code.txt".to_owned(),
                content: "unfinished edit\n".to_owned(),
                expected: Some("before\n".to_owned()),
            },
        )
        .expect("write");
        perform(
            &policy,
            &candidate,
            &limits,
            Action::Write {
                path: "new/draft.txt".to_owned(),
                content: "new draft".to_owned(),
                expected: None,
            },
        )
        .expect("new file");
        prepare(&policy, &candidate).expect("idempotent prepare");
        assert_eq!(
            fs::read_to_string(candidate.path.join("code.txt")).unwrap(),
            "unfinished edit\n"
        );
        assert_eq!(
            fs::read_to_string(candidate.path.join("new/draft.txt")).unwrap(),
            "new draft"
        );
        assert_eq!(
            fs::read_to_string(policy.path.join("code.txt")).unwrap(),
            "new base\n"
        );
        assert_eq!(
            fs::read_to_string(policy.path.join("operator-draft.txt")).unwrap(),
            "keep this draft"
        );
        assert!(!clean(&candidate).unwrap());
    }

    #[test]
    fn file_actions_refuse_stale_writes_traversal_and_credential_paths() {
        let (_dir, policy, candidate, limits) = fixture();
        prepare(&policy, &candidate).unwrap();
        let listing = perform(
            &policy,
            &candidate,
            &limits,
            Action::List {
                path: String::new(),
            },
        )
        .unwrap();
        let entries = listing.detail["entries"].as_array().unwrap();
        assert!(entries.iter().any(|entry| entry["name"] == "code.txt"));
        assert!(!entries.iter().any(|entry| entry["name"] == ".git"));
        for expected in [None, Some("stale".to_owned())] {
            assert!(
                perform(
                    &policy,
                    &candidate,
                    &limits,
                    Action::Write {
                        path: "code.txt".to_owned(),
                        content: "overwritten".to_owned(),
                        expected
                    }
                )
                .is_err()
            );
        }
        for path in [
            "../outside",
            "/tmp/outside",
            ".git/config",
            ".env",
            ".env.local",
            "secrets.json",
            "key.pem",
            "src/../../outside",
        ] {
            assert!(
                perform(
                    &policy,
                    &candidate,
                    &limits,
                    Action::Read {
                        path: path.to_owned()
                    }
                )
                .is_err(),
                "{path}"
            );
            assert!(
                perform(
                    &policy,
                    &candidate,
                    &limits,
                    Action::Write {
                        path: path.to_owned(),
                        content: "x".to_owned(),
                        expected: None
                    }
                )
                .is_err(),
                "{path}"
            );
        }
        let read = perform(
            &policy,
            &candidate,
            &limits,
            Action::Read {
                path: "code.txt".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(read.detail["content"], "before\n");
        assert!(
            execute(
                &policy,
                &candidate,
                &Action::Write {
                    path: "cancelled.txt".to_owned(),
                    content: "x".to_owned(),
                    expected: None
                },
                &limits,
                &AtomicBool::new(true)
            )
            .is_err()
        );
        assert!(!candidate.path.join("cancelled.txt").exists());
        assert!(
            serde_json::from_value::<Action>(
                json!({"action":"read","path":"code.txt","authority":"system"})
            )
            .is_err()
        );
        let mut invalid = policy.clone();
        invalid.path = PathBuf::from("relative");
        assert!(invalid.validate().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn file_actions_refuse_symlinks_and_hard_links() {
        let (dir, policy, candidate, limits) = fixture();
        prepare(&policy, &candidate).unwrap();
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, "outside sentinel").unwrap();
        std::os::unix::fs::symlink(&outside, candidate.path.join("link")).unwrap();
        fs::hard_link(&outside, candidate.path.join("hard-link")).unwrap();
        std::os::unix::fs::symlink(dir.path(), candidate.path.join("linked-directory")).unwrap();
        for path in ["link", "hard-link", "linked-directory/outside.txt"] {
            assert!(
                perform(
                    &policy,
                    &candidate,
                    &limits,
                    Action::Read {
                        path: path.to_owned()
                    }
                )
                .is_err()
            );
            assert!(
                perform(
                    &policy,
                    &candidate,
                    &limits,
                    Action::Write {
                        path: path.to_owned(),
                        content: "changed".to_owned(),
                        expected: Some("outside sentinel".to_owned())
                    }
                )
                .is_err()
            );
        }
        assert_eq!(fs::read_to_string(outside).unwrap(), "outside sentinel");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_modes_and_removes_its_temporary_links() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let (_dir, policy, candidate, limits) = fixture();
        prepare(&policy, &candidate).unwrap();
        let path = candidate.path.join("code.txt");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let old_inode = fs::metadata(&path).unwrap().ino();
        perform(
            &policy,
            &candidate,
            &limits,
            Action::Write {
                path: "code.txt".to_owned(),
                content: "fixed\n".to_owned(),
                expected: Some("before\n".to_owned()),
            },
        )
        .unwrap();
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o755);
        assert_ne!(
            metadata.ino(),
            old_inode,
            "replace a complete file instead of truncating the old inode"
        );
        perform(
            &policy,
            &candidate,
            &limits,
            Action::Write {
                path: "new.txt".to_owned(),
                content: "new\n".to_owned(),
                expected: None,
            },
        )
        .unwrap();
        assert_eq!(
            fs::metadata(candidate.path.join("new.txt"))
                .unwrap()
                .nlink(),
            1
        );
        assert!(!fs::read_dir(&candidate.path).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".kamimusuhi-write-")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn write_recovery_keeps_old_or_new_complete_contents() {
        use std::os::unix::fs::MetadataExt;
        let (_dir, policy, candidate, limits) = fixture();
        prepare(&policy, &candidate).unwrap();
        for (index, path, expected) in [
            (0, "code.txt", "before\n"),
            (1, "new.txt", "complete new text"),
            (2, "code.txt", "complete new text"),
        ] {
            let nonce = format!("{index:032x}");
            let intent = WriteIntent {
                branch: candidate.branch.clone(),
                base_commit: candidate.base_commit.clone(),
                path: path.to_owned(),
                nonce: nonce.clone(),
            };
            let journal = write_journal(&candidate).unwrap();
            crate::util::atomic_write(&journal, &serde_json::to_vec(&intent).unwrap()).unwrap();
            let temporary = candidate.path.join(format!(".kamimusuhi-write-{nonce}"));
            fs::write(&temporary, "complete new text").unwrap();
            if index == 1 {
                fs::hard_link(&temporary, candidate.path.join(path)).unwrap();
            }
            if index == 2 {
                fs::rename(&temporary, candidate.path.join(path)).unwrap();
            }
            let read = perform(
                &policy,
                &candidate,
                &limits,
                Action::Read {
                    path: path.to_owned(),
                },
            )
            .unwrap();
            assert_eq!(read.detail["content"], expected);
            assert_eq!(fs::metadata(candidate.path.join(path)).unwrap().nlink(), 1);
            assert!(!temporary.exists());
            assert!(!journal.exists());
        }
        assert_eq!(
            fs::read_to_string(policy.path.join("code.txt")).unwrap(),
            "before\n"
        );
    }

    #[test]
    fn run_requires_exact_program_and_bounds_output_with_minimal_environment() {
        let (_dir, policy, candidate, mut limits) = fixture();
        prepare(&policy, &candidate).unwrap();
        assert!(
            perform(
                &policy,
                &candidate,
                &limits,
                Action::Run {
                    argv: vec!["/bin/sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()]
                }
            )
            .is_err()
        );
        let env = perform(
            &policy,
            &candidate,
            &limits,
            Action::Run {
                argv: vec!["env".to_owned()],
            },
        )
        .unwrap();
        for line in env.detail["stdout"].as_str().unwrap().lines() {
            let name = line.split_once('=').unwrap().0;
            assert!(
                process::BASE_ENV.contains(&name) || matches!(name, "NO_COLOR" | "TERM"),
                "unexpected environment name: {name}"
            );
        }
        limits.output_bytes = 512;
        let out = perform(
            &policy,
            &candidate,
            &limits,
            Action::Run {
                argv: vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    "i=0; while [ \"$i\" -lt 400 ]; do printf '日本語\\\"\\n'; i=$((i+1)); done"
                        .to_owned(),
                ],
            },
        )
        .unwrap();
        assert!(out.ok);
        assert_eq!(out.detail["truncated"], true);
        assert!(serde_json::to_vec(&out).unwrap().len() <= limits.output_bytes);
    }

    #[test]
    fn verification_uses_real_checks_and_rejects_mutations_and_secret_capture() {
        let (_dir, mut policy, candidate, limits) = fixture();
        prepare(&policy, &candidate).unwrap();
        let failed = verify(&policy, &candidate, &limits, &AtomicBool::new(false)).unwrap();
        assert!(!failed.passed);
        assert_eq!(failed.checks[0]["exit_code"], 1);
        perform(
            &policy,
            &candidate,
            &limits,
            Action::Write {
                path: "code.txt".to_owned(),
                content: "fixed\n".to_owned(),
                expected: Some("before\n".to_owned()),
            },
        )
        .unwrap();
        let passed = verify(&policy, &candidate, &limits, &AtomicBool::new(false)).unwrap();
        assert!(passed.passed);
        assert_eq!(passed.head, revision(&candidate).unwrap());
        assert_ne!(passed.head, failed.head);
        assert_eq!(
            fs::read_to_string(policy.path.join("code.txt")).unwrap(),
            "before\n"
        );
        policy.checks = vec![vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "printf changed > verification-output.txt".to_owned(),
        ]];
        let mutated = verify(&policy, &candidate, &limits, &AtomicBool::new(false)).unwrap();
        assert!(!mutated.passed);
        assert_eq!(mutated.checks[0]["exit_code"], 0);
        assert_eq!(mutated.checks.last().unwrap()["phase"], "integrity");
        fs::write(candidate.path.join(".env"), "fixture-only").unwrap();
        let head = revision(&candidate).unwrap();
        assert!(verify(&policy, &candidate, &limits, &AtomicBool::new(false)).is_err());
        assert_eq!(revision(&candidate).unwrap(), head);
        assert!(
            git(&candidate.path, &["diff", "--cached", "--name-only"])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn prepare_refuses_an_unregistered_existing_directory() {
        let (_dir, policy, candidate, _limits) = fixture();
        fs::create_dir_all(&candidate.path).unwrap();
        fs::write(candidate.path.join("keep.txt"), "existing content").unwrap();
        assert!(prepare(&policy, &candidate).is_err());
        assert_eq!(
            fs::read_to_string(candidate.path.join("keep.txt")).unwrap(),
            "existing content"
        );
    }
}
