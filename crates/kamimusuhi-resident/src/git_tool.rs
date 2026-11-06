//! Typed Git actions over operator-registered repositories, never a shell.
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::state::Shared;
use kamimusuhi_runtime::agent_exec::process::{self, ProcessSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryConfig {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub allow_write: bool,
    #[serde(default)]
    pub allow_push: bool,
}

impl RepositoryConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || self.name.len() > 64
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            || !self.path.is_absolute()
            || (self.allow_push && !self.allow_write)
        {
            return Err("git repository name/path/permissions are invalid".into());
        }
        Ok(())
    }
}

pub fn definitions() -> Vec<Value> {
    vec![json!({"type":"function","function":{
        "name":"git",
        "description":"Git in an operator-registered repository. list shows names and permissions; status/diff/log/branches inspect it. Explicit file paths are required for add and commit; commit changes only those files and preserves other staged files. create_branch does not checkout; checkout and pull require a clean tree; pull is ff-only. fetch/push use the existing origin only; push needs allow_push and never forces. Shell, arbitrary git arguments, hooks, external diff/textconv/filters, reset, clean and force operations are unavailable. Sensitive credential paths are excluded from content views and refused for writes.",
        "parameters":{"type":"object","additionalProperties":false,"properties":{
            "action":{"type":"string","enum":["list","status","diff","log","branches","create_branch","checkout","add","commit","fetch","pull","push"]},
            "repository":{"type":"string"},"paths":{"type":"array","maxItems":32,"items":{"type":"string"}},
            "branch":{"type":"string"},"message":{"type":"string","description":"Conventional Commit message; required for commit"},
            "staged":{"type":"boolean"},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["action"]}
    }})]
}

fn fail(message: &str) -> (u16, Value) {
    (200, json!({"ok":false,"error":message}))
}
fn mutates(action: &str) -> bool {
    matches!(
        action,
        "create_branch" | "checkout" | "add" | "commit" | "fetch" | "pull" | "push"
    )
}

pub fn call(shared: &Shared, args: &Value) -> (u16, Value) {
    if args["action"] == "list" {
        return (
            200,
            json!({"ok":true,"result":{"repositories":shared.config.tools.git_repositories.iter().map(|r|json!({"name":r.name,"allow_write":r.allow_write,"allow_push":r.allow_push})).collect::<Vec<_>>()}}),
        );
    }
    let Some(repo) = shared
        .config
        .tools
        .git_repositories
        .iter()
        .find(|r| Some(r.name.as_str()) == args["repository"].as_str())
    else {
        return fail("repository is not registered");
    };
    let action = args["action"].as_str().unwrap_or("");
    let _guard = if mutates(action) {
        match shared.workspace_mutation.try_lock() {
            Ok(g) => Some(g),
            Err(_) => return fail("workspace validation/application in progress"),
        }
    } else {
        None
    };
    match execute(repo, args) {
        Ok(result) => (200, json!({"ok":true,"result":result})),
        Err(e) => fail(e),
    }
}

/// Known credential paths are never treated as source content. Status can
/// still report their names so the operator sees the actual tree state.
fn sensitive(path: &Path) -> bool {
    path.components().any(|c| {
        let Component::Normal(part) = c else {
            return false;
        };
        let s = part.to_string_lossy().to_ascii_lowercase();
        s == ".git"
            || s.ends_with(".git")
            || s == ".env"
            || s.starts_with(".env.")
            || s == "private"
            || s == ".ssh"
            || s == ".aws"
            || s == ".gnupg"
            || s.starts_with("secret")
            || s.ends_with(".token")
            || s.ends_with(".key")
            || s.ends_with(".pem")
            || s.ends_with(".p12")
            || s.ends_with(".pfx")
            || s == "token"
            || s == "token.json"
            || s == "token.txt"
            || s == "key"
            || s == "keys"
            || s == "id_rsa"
            || s == "id_ed25519"
            || s == "credentials"
            || s == "credentials.json"
    })
}

fn safe_path(root: &Path, input: &str) -> Result<String, &'static str> {
    if input.is_empty() || input.len() > 1024 || input.chars().any(char::is_control) {
        return Err("invalid file path");
    }
    let path = Path::new(input);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
        || sensitive(path)
    {
        return Err("path is outside source files or is sensitive");
    }
    let mut at = root.to_path_buf();
    for part in path.components() {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => return Err("symlink paths are not supported"),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("cannot inspect file path"),
        }
    }
    if at.is_dir() {
        return Err("select individual files, not directories");
    }
    Ok(input.to_owned())
}

fn branch(input: &str) -> Result<&str, &'static str> {
    if input.is_empty()
        || input.len() > 200
        || input.starts_with(['-', '/', '.'])
        || input.ends_with(['/', '.'])
        || input.contains("..")
        || input.contains("//")
        || input.ends_with(".lock")
        || !input
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
    {
        return Err("invalid branch name");
    }
    Ok(input)
}

fn conventional(message: &str) -> bool {
    if message.len() > 4096 || message.contains('\0') {
        return false;
    }
    let first = message.lines().next().unwrap_or("");
    let Some((kind, subject)) = first.split_once(": ") else {
        return false;
    };
    let kind = kind.strip_suffix('!').unwrap_or(kind);
    let base = kind.split('(').next().unwrap_or("");
    let scope_ok = if let Some((_, scope)) = kind.split_once('(') {
        scope.ends_with(')')
            && scope.len() > 1
            && scope[..scope.len() - 1]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
    } else {
        true
    };
    !subject.trim().is_empty()
        && scope_ok
        && [
            "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore",
            "revert",
        ]
        .contains(&base)
}

fn sanitized(text: &str) -> String {
    text.split_inclusive(char::is_whitespace)
        .map(|word| {
            let trimmed = word.trim_end();
            let tail = &word[trimmed.len()..];
            if trimmed.contains("://")
                && trimmed
                    .split("://")
                    .nth(1)
                    .is_some_and(|v| v.contains('@') || v.contains('?'))
                || ["ghp_", "github_pat_", "gho_", "sk-"]
                    .iter()
                    .any(|prefix| trimmed.contains(prefix))
                || trimmed.to_ascii_lowercase().starts_with("authorization:")
            {
                format!("[REDACTED]{tail}")
            } else {
                word.to_owned()
            }
        })
        .collect()
}

fn clipped(text: &str) -> (String, bool) {
    let mut end = text.len().min(32 * 1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), end < text.len())
}

/// Fixed options disable implicit programs for source operations. Existing
/// credential helpers remain available for explicitly permitted network Git.
fn base() -> Vec<String> {
    [
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.pager=cat",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "tag.gpgsign=false",
        "-c",
        "credential.interactive=false",
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "submodule.recurse=false",
        "-c",
        "core.sshCommand=ssh -oBatchMode=yes",
    ]
    .iter()
    .map(|s| (*s).into())
    .collect()
}

fn run(root: &Path, args: &[String], timeout: Duration) -> Result<String, &'static str> {
    let output = process::run(
        &ProcessSpec {
            program: "git",
            args,
            cwd: Some(root),
            env_passthrough: &[],
            extra_env: &[
                ("GIT_TERMINAL_PROMPT".into(), "0".into()),
                ("GIT_PAGER".into(), "cat".into()),
                ("GIT_EDITOR".into(), "false".into()),
            ],
            timeout,
        },
        &AtomicBool::new(false),
        |_| {},
    )
    .map_err(|_| "could not start git")?;
    if output.success() {
        Ok(output.stdout)
    } else {
        Err(
            "git operation failed; inspect repository state locally (raw diagnostics are not returned)",
        )
    }
}

fn git(root: &Path, options: &[String], args: &[&str]) -> Result<String, &'static str> {
    let mut all = options.to_vec();
    all.extend(args.iter().map(|s| (*s).to_owned()));
    run(root, &all, Duration::from_secs(60))
}

fn origin(root: &Path, options: &[String]) -> Result<(), &'static str> {
    let raw = git(root, options, &["remote", "get-url", "origin"])?;
    let url = raw.trim();
    if url.is_empty()
        || url.chars().any(char::is_control)
        || url.starts_with('-')
        || url.contains("::")
    {
        return Err("origin URL uses an unsupported transport");
    }
    if let Some(rest) = url.strip_prefix("https://") {
        let authority = rest.split('/').next().unwrap_or("");
        if authority.is_empty() || authority.contains('@') || url.contains(['?', '#']) {
            return Err("origin must not embed credentials");
        }
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        let authority = rest.split('/').next().unwrap_or("");
        if authority.is_empty()
            || authority.contains([' ', '\t', '$', '`'])
            || authority.split('@').next().is_some_and(|u| u.contains(':'))
        {
            return Err("invalid SSH origin");
        }
    } else if url.contains("://") && !url.starts_with("file://") {
        return Err("origin URL uses an unsupported transport");
    }
    Ok(())
}

fn execute(repo: &RepositoryConfig, args: &Value) -> Result<Value, &'static str> {
    let map = args.as_object().ok_or("arguments must be an object")?;
    if map.keys().any(|k| {
        ![
            "action",
            "repository",
            "paths",
            "branch",
            "message",
            "staged",
            "limit",
        ]
        .contains(&k.as_str())
    }) {
        return Err("unsupported git argument");
    }
    let action = args["action"].as_str().ok_or("action is required")?;
    if ![
        "status",
        "diff",
        "log",
        "branches",
        "create_branch",
        "checkout",
        "add",
        "commit",
        "fetch",
        "pull",
        "push",
    ]
    .contains(&action)
    {
        return Err("unsupported git action");
    }
    if mutates(action) && !repo.allow_write {
        return Err("repository is read-only");
    }
    if action == "push" && !repo.allow_push {
        return Err("push is not enabled for this repository");
    }
    let root = repo
        .path
        .canonicalize()
        .map_err(|_| "repository path is unavailable")?;
    let mut options = base();
    let top = git(&root, &options, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "registered path is not a Git repository")?;
    if Path::new(top.trim()).canonicalize().ok() != Some(root.clone()) {
        return Err("registered path must be the Git repository root");
    }
    // Disable each configured clean/smudge/process filter without reading its values.
    if let Ok(names) = git(
        &root,
        &options,
        &["config", "--name-only", "--get-regexp", "^filter\\."],
    ) {
        let mut filters = std::collections::BTreeSet::new();
        for key in names.lines() {
            if let Some((name, _)) = key.rsplit_once('.') {
                filters.insert(name.to_owned());
            }
        }
        for name in filters {
            for (suffix, value) in [
                ("clean", ""),
                ("smudge", ""),
                ("process", ""),
                ("required", "false"),
            ] {
                options.push("-c".into());
                options.push(format!("{name}.{suffix}={value}"));
            }
        }
    }
    let paths = if let Some(raw) = args.get("paths") {
        let paths = raw.as_array().ok_or("paths must be an array")?;
        if paths.len() > 32 {
            return Err("at most 32 explicit paths are supported");
        }
        paths
            .iter()
            .map(|v| safe_path(&root, v.as_str().ok_or("paths must contain strings")?))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![]
    };
    let literal: Vec<String> = paths.iter().map(|p| format!(":(literal){p}")).collect();
    let mut command: Vec<String> = Vec::new();
    let mut network = false;
    match action {
        "status" => {
            command.extend(["status", "--short", "--untracked-files=normal"].map(str::to_owned))
        }
        "branches" => {
            command.extend(["branch", "--list", "--format=%(refname:short)"].map(str::to_owned))
        }
        "log" => {
            let limit = args.get("limit").map_or(Ok(10), |v| {
                v.as_u64()
                    .filter(|n| (1..=50).contains(n))
                    .ok_or("limit must be 1..50")
            })?;
            command.extend(
                [
                    "log",
                    "--no-decorate",
                    "--format=%h %ad %s",
                    "--date=iso-strict",
                ]
                .map(str::to_owned),
            );
            command.push(format!("-{limit}"));
        }
        "diff" => {
            if args.get("staged").is_some_and(|v| !v.is_boolean()) {
                return Err("staged must be boolean");
            }
            let staged = args["staged"] == true;
            let files = if paths.is_empty() {
                let mut names = vec!["diff", "--name-only", "-z"];
                if staged {
                    names.push("--cached");
                }
                names.push("--");
                git(&root, &options, &names)?
                    .split('\0')
                    .filter(|s| !s.is_empty())
                    .filter_map(|p| safe_path(&root, p).ok())
                    .map(|p| format!(":(literal){p}"))
                    .collect()
            } else {
                literal
            };
            if files.is_empty() {
                return Ok(
                    json!({"action":action,"repository":repo.name,"stdout":"","truncated":false}),
                );
            }
            command.extend(["diff", "--no-ext-diff", "--no-textconv"].map(str::to_owned));
            if staged {
                command.push("--cached".into());
            }
            command.push("--".into());
            command.extend(files);
        }
        "create_branch" => {
            command.extend(
                [
                    "branch",
                    branch(args["branch"].as_str().ok_or("branch is required")?)?,
                ]
                .map(str::to_owned),
            );
        }
        "checkout" => {
            if !git(&root, &options, &["status", "--porcelain"])?
                .trim()
                .is_empty()
            {
                return Err("checkout requires a clean tree; existing edits are preserved");
            }
            command.extend(
                [
                    "switch",
                    branch(args["branch"].as_str().ok_or("branch is required")?)?,
                ]
                .map(str::to_owned),
            );
        }
        "add" | "commit" => {
            if literal.is_empty() {
                return Err("explicit file paths are required");
            }
            if action == "add" {
                command.extend(["add", "--"].map(str::to_owned));
            } else {
                let message = args["message"]
                    .as_str()
                    .ok_or("commit message is required")?;
                if !conventional(message) {
                    return Err("commit message must use Conventional Commits");
                }
                command.extend(
                    ["commit", "--only", "--no-verify", "-m", message, "--"].map(str::to_owned),
                );
            }
            command.extend(literal);
        }
        "fetch" | "pull" | "push" => {
            origin(&root, &options)?;
            network = true;
            match action {
                "fetch" => command
                    .extend(["fetch", "--no-recurse-submodules", "origin"].map(str::to_owned)),
                "pull" => {
                    if !git(&root, &options, &["status", "--porcelain"])?
                        .trim()
                        .is_empty()
                    {
                        return Err("pull requires a clean tree; existing edits are preserved");
                    }
                    let current = git(&root, &options, &["symbolic-ref", "--short", "HEAD"])?;
                    branch(current.trim())?;
                    command.extend(
                        [
                            "pull",
                            "--ff-only",
                            "--no-rebase",
                            "--no-recurse-submodules",
                            "origin",
                            current.trim(),
                        ]
                        .map(str::to_owned),
                    );
                }
                _ => {
                    let current = git(&root, &options, &["symbolic-ref", "--short", "HEAD"])?;
                    branch(current.trim())?;
                    command.extend(
                        [
                            "push",
                            "--porcelain",
                            "--no-verify",
                            "--no-recurse-submodules",
                            "origin",
                        ]
                        .map(str::to_owned),
                    );
                    command.push(format!("HEAD:refs/heads/{}", current.trim()));
                }
            }
        }
        _ => unreachable!(),
    }
    options.extend(command);
    let output = run(
        &root,
        &options,
        Duration::from_secs(if network { 120 } else { 60 }),
    )?;
    let content = if matches!(action, "diff" | "log") {
        output
            .lines()
            .map(|line| {
                let lower = line.to_ascii_lowercase();
                if [
                    "authorization",
                    "password",
                    "secret",
                    "api_key",
                    "api-key",
                    "access_token",
                    "bearer ",
                ]
                .iter()
                .any(|key| lower.contains(key))
                {
                    "[REDACTED credential-shaped line]"
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        output
    };
    let (stdout, truncated) = if network {
        ("origin operation completed".into(), false)
    } else {
        clipped(&sanitized(&content))
    };
    Ok(json!({"action":action,"repository":repo.name,"stdout":stdout,"truncated":truncated}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn raw(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git fixture {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    fn fixture() -> (tempfile::TempDir, RepositoryConfig) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        raw(&root, &["init", "-q", "-b", "main"]);
        raw(&root, &["config", "user.name", "Fixture"]);
        raw(&root, &["config", "user.email", "fixture@local"]);
        std::fs::write(root.join("a.txt"), "a1\n").unwrap();
        std::fs::write(root.join("b.txt"), "b1\n").unwrap();
        std::fs::write(root.join(".env"), "FIXTURE_ONLY=old\n").unwrap();
        raw(&root, &["add", "a.txt", "b.txt", ".env"]);
        raw(
            &root,
            &[
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-qm",
                "test: initial",
            ],
        );
        let repo = RepositoryConfig {
            name: "fixture".into(),
            path: root,
            allow_write: true,
            allow_push: false,
        };
        (dir, repo)
    }

    #[test]
    fn git_commit_only_named_files_preserves_other_staged_edits() {
        let (_dir, repo) = fixture();
        std::fs::write(repo.path.join("a.txt"), "a2\n").unwrap();
        std::fs::write(repo.path.join("b.txt"), "b2\n").unwrap();
        raw(&repo.path, &["add", "b.txt"]);
        execute(&repo, &json!({"action":"add","paths":["a.txt"]})).unwrap();
        execute(
            &repo,
            &json!({"action":"commit","paths":["a.txt"],"message":"fix: improve a"}),
        )
        .unwrap();
        assert_eq!(raw(&repo.path, &["show", "HEAD:a.txt"]), "a2\n");
        assert_eq!(raw(&repo.path, &["show", "HEAD:b.txt"]), "b1\n");
        assert_eq!(
            raw(&repo.path, &["diff", "--cached", "--name-only"]).trim(),
            "b.txt"
        );
        assert!(execute(&repo, &json!({"action":"checkout","branch":"other"})).is_err());
        assert_eq!(
            std::fs::read_to_string(repo.path.join("b.txt")).unwrap(),
            "b2\n"
        );
    }

    #[test]
    fn git_content_views_exclude_sensitive_paths_and_writes_refuse_escapes() {
        let (dir, repo) = fixture();
        std::fs::write(repo.path.join("a.txt"), "visible-change\n").unwrap();
        std::fs::write(repo.path.join(".env"), "FIXTURE_ONLY=must-not-appear\n").unwrap();
        let diff = execute(&repo, &json!({"action":"diff"})).unwrap();
        assert!(diff["stdout"].as_str().unwrap().contains("visible-change"));
        assert!(!diff.to_string().contains("must-not-appear"));
        for path in [
            ".env",
            "../outside",
            "/etc/passwd",
            ".git/config",
            "private/key.txt",
        ] {
            assert!(
                execute(&repo, &json!({"action":"add","paths":[path]})).is_err(),
                "{path}"
            );
        }
        #[cfg(unix)]
        {
            std::fs::write(dir.path().join("outside"), "private fixture").unwrap();
            std::os::unix::fs::symlink(dir.path(), repo.path.join("link")).unwrap();
            assert!(execute(&repo, &json!({"action":"add","paths":["link/outside"]})).is_err());
        }
        assert!(execute(&repo, &json!({"action":"reset"})).is_err());
        assert!(execute(&repo, &json!({"action":"push","force":true})).is_err());
        assert!(execute(&repo, &json!({"action":"create_branch","branch":"--force"})).is_err());
        assert!(
            execute(
                &repo,
                &json!({"action":"commit","paths":["a.txt"],"message":"ordinary message"})
            )
            .is_err()
        );
    }

    #[test]
    fn git_local_origin_push_fetch_pull_and_clean_checkout_work() {
        let (dir, mut repo) = fixture();
        let bare = dir.path().join("remote.git");
        std::fs::create_dir(&bare).unwrap();
        raw(&bare, &["init", "--bare", "-q"]);
        raw(
            &repo.path,
            &["remote", "add", "origin", bare.to_str().unwrap()],
        );
        assert!(execute(&repo, &json!({"action":"push"})).is_err());
        repo.allow_push = true;
        execute(&repo, &json!({"action":"push"})).unwrap();
        execute(&repo, &json!({"action":"fetch"})).unwrap();
        execute(&repo, &json!({"action":"pull"})).unwrap();
        execute(
            &repo,
            &json!({"action":"create_branch","branch":"codex/fixture"}),
        )
        .unwrap();
        execute(
            &repo,
            &json!({"action":"checkout","branch":"codex/fixture"}),
        )
        .unwrap();
        assert_eq!(
            raw(&repo.path, &["branch", "--show-current"]).trim(),
            "codex/fixture"
        );
        let branches = execute(&repo, &json!({"action":"branches"})).unwrap();
        assert!(branches["stdout"].as_str().unwrap().contains("main"));
        let log = execute(&repo, &json!({"action":"log","limit":1})).unwrap();
        assert!(log["stdout"].as_str().unwrap().contains("test: initial"));
    }

    #[test]
    fn git_hooks_filters_and_remote_credentials_do_not_escape() {
        let (_dir, mut repo) = fixture();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let hook = repo.path.join(".git/hooks/pre-commit");
            std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
            std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        raw(
            &repo.path,
            &[
                "config",
                "filter.fixture.clean",
                "sh -c 'echo invoked > filter-ran; cat'",
            ],
        );
        raw(&repo.path, &["config", "filter.fixture.required", "true"]);
        std::fs::write(repo.path.join(".gitattributes"), "*.txt filter=fixture\n").unwrap();
        std::fs::write(repo.path.join("a.txt"), "new\n").unwrap();
        execute(&repo, &json!({"action":"add","paths":["a.txt"]})).unwrap();
        execute(&repo,&json!({"action":"commit","paths":["a.txt"],"message":"fix: bypass fixture hook safely"})).unwrap();
        assert!(!repo.path.join("filter-ran").exists());
        raw(
            &repo.path,
            &[
                "remote",
                "add",
                "origin",
                "https://fixture:must-not-echo@example.invalid/repo.git",
            ],
        );
        repo.allow_push = true;
        let error = execute(&repo, &json!({"action":"push"})).unwrap_err();
        assert!(!error.contains("must-not-echo"));
        assert!(sanitized("https://user:pass@host/repo ghp_fixture-secret").contains("[REDACTED]"));
    }

    #[test]
    fn git_read_only_and_non_root_repositories_fail_closed() {
        let (_dir, mut repo) = fixture();
        repo.allow_write = false;
        assert!(execute(&repo, &json!({"action":"status"})).is_ok());
        assert!(execute(&repo, &json!({"action":"add","paths":["a.txt"]})).is_err());
        let sub = repo.path.join("sub");
        std::fs::create_dir(&sub).unwrap();
        repo.path = sub;
        assert!(execute(&repo, &json!({"action":"status"})).is_err());
        assert!(
            RepositoryConfig {
                name: "bad".into(),
                path: PathBuf::from("relative"),
                allow_write: false,
                allow_push: false
            }
            .validate()
            .is_err()
        );
    }
}
