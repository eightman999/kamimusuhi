//! Isolated git worktrees for tasks that may write.
//!
//! A write task never runs in the workspace itself. It gets its own
//! worktree under `<root>/worktrees/<workspace>/<task>` on a branch
//! `kamimusuhi/task/<task>` started from the workspace's `HEAD`. When the
//! harness finishes, whatever it left uncommitted is committed to that
//! branch and the diff is saved as a patch, so a failed or unwanted task
//! costs nothing but a branch. Bringing the work into the workspace
//! (`apply`, a `--no-ff` merge into a clean tree) and throwing it away
//! (`discard`) are explicit operator actions.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::util::run_with_timeout;

const GIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Commits made by the resident on a task branch.
const AUTHOR: [&str; 6] = [
    "-c",
    "user.name=Kamimusuhi Task",
    "-c",
    "user.email=task@kamimusuhi.local",
    "-c",
    "commit.gpgsign=false",
];

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let dir = dir.to_string_lossy();
    let mut all = vec!["-C", dir.as_ref()];
    all.extend_from_slice(args);
    let out = run_with_timeout("git", &all, GIT_TIMEOUT)
        .ok_or_else(|| format!("git {} timed out", args.join(" ")))?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(format!(
            "git {}: {}",
            args.first().copied().unwrap_or(""),
            out.stderr.trim().lines().last().unwrap_or("failed")
        ))
    }
}

/// A task's worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    /// Commit the branch started from.
    pub base: String,
}

pub fn branch_for(task_id: &str) -> String {
    format!("kamimusuhi/task/{task_id}")
}

/// Whether `dir` is the top of a git work tree.
pub fn is_repo(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--show-toplevel"])
        .ok()
        .and_then(|top| PathBuf::from(top.trim()).canonicalize().ok())
        .is_some_and(|top| dir.canonicalize().is_ok_and(|d| d == top))
}

/// Create a worktree for `task_id` from the workspace's `HEAD`.
pub fn create(workspace: &Path, root: &Path, task_id: &str) -> Result<Worktree, String> {
    if !is_repo(workspace) {
        return Err(
            "書き込みタスクの workspace は git リポジトリの最上位である必要がある".to_owned(),
        );
    }
    let base = git(workspace, &["rev-parse", "HEAD"])?.trim().to_owned();
    let path = root.join(task_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("worktree dir: {e}"))?;
    }
    let branch = branch_for(task_id);
    let path_text = path.to_string_lossy().into_owned();
    git(
        workspace,
        &["worktree", "add", "-q", "-b", &branch, &path_text, &base],
    )?;
    Ok(Worktree { path, branch, base })
}

/// Commit what the harness left and describe the whole branch.
pub struct Captured {
    pub files: Vec<String>,
    pub diff_stat: String,
    pub patch: String,
    pub head: String,
}

pub fn capture(wt: &Worktree, message: &str) -> Result<Captured, String> {
    git(&wt.path, &["add", "-A"])?;
    let staged = git(&wt.path, &["diff", "--cached", "--name-only"])?;
    if !staged.trim().is_empty() {
        let mut args: Vec<&str> = AUTHOR.to_vec();
        args.extend_from_slice(&["commit", "-q", "--no-verify", "-m", message]);
        git(&wt.path, &args)?;
    }
    let range = format!("{}..HEAD", wt.base);
    let files = git(&wt.path, &["diff", "--name-only", &range])?
        .lines()
        .map(str::to_owned)
        .collect();
    Ok(Captured {
        files,
        diff_stat: git(&wt.path, &["diff", "--stat", &range])?
            .trim()
            .to_owned(),
        patch: git(&wt.path, &["diff", "--no-ext-diff", "--binary", &range])?,
        head: git(&wt.path, &["rev-parse", "HEAD"])?.trim().to_owned(),
    })
}

/// Merge the task branch into the workspace. Refused unless the
/// workspace tree is clean, so no one's uncommitted work is touched.
pub fn apply(workspace: &Path, branch: &str, message: &str) -> Result<String, String> {
    let dirty = git(
        workspace,
        &["status", "--porcelain", "--untracked-files=no"],
    )?;
    if !dirty.trim().is_empty() {
        return Err(
            "workspace に未コミットの変更があるため反映しない（commit/stash してから再実行）"
                .to_owned(),
        );
    }
    let mut args: Vec<&str> = AUTHOR.to_vec();
    args.extend_from_slice(&["merge", "--no-ff", "--no-edit", "-m", message, branch]);
    if let Err(e) = git(workspace, &args) {
        let _ = git(workspace, &["merge", "--abort"]);
        return Err(format!("merge できない（中止した）: {e}"));
    }
    Ok(git(workspace, &["rev-parse", "HEAD"])?.trim().to_owned())
}

/// Remove the worktree; with `delete_branch`, the branch too.
pub fn remove(
    workspace: &Path,
    path: &Path,
    branch: &str,
    delete_branch: bool,
) -> Result<(), String> {
    if path.exists() {
        git(
            workspace,
            &["worktree", "remove", "--force", &path.to_string_lossy()],
        )?;
    }
    let _ = git(workspace, &["worktree", "prune"]);
    if delete_branch {
        git(workspace, &["branch", "-D", branch])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn init_repo(dir: &Path) {
        for args in [
            vec!["init", "-q"],
            vec![
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
        ] {
            git(dir, &args).expect("git");
        }
    }

    #[test]
    fn a_task_writes_only_its_worktree_and_can_be_applied_or_discarded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).expect("ws");
        init_repo(&ws);
        let wt = create(&ws, &dir.path().join("wt"), "t1").expect("worktree");
        assert_eq!(wt.branch, "kamimusuhi/task/t1");
        std::fs::write(wt.path.join("new.txt"), "hello\n").expect("write");
        assert!(!ws.join("new.txt").exists(), "workspace untouched");
        let captured = capture(&wt, "task t1").expect("capture");
        assert_eq!(captured.files, vec!["new.txt"]);
        assert!(captured.patch.contains("+hello"));
        assert!(captured.diff_stat.contains("new.txt"));

        // A dirty workspace refuses the merge…
        std::fs::write(ws.join("dirty.txt"), "x").expect("dirty");
        git(&ws, &["add", "dirty.txt"]).expect("add");
        assert!(apply(&ws, &wt.branch, "apply t1").is_err());
        git(&ws, &["reset", "-q"]).expect("reset");
        std::fs::remove_file(ws.join("dirty.txt")).expect("rm");
        // …a clean one takes it.
        apply(&ws, &wt.branch, "apply t1").expect("apply");
        assert_eq!(
            std::fs::read_to_string(ws.join("new.txt")).expect("merged"),
            "hello\n"
        );
        remove(&ws, &wt.path, &wt.branch, true).expect("remove");
        assert!(!wt.path.exists());
        assert!(git(&ws, &["rev-parse", "--verify", &wt.branch]).is_err());

        // Not a repository top: refused.
        let sub = ws.join("sub");
        std::fs::create_dir(&sub).expect("sub");
        assert!(create(&sub, &dir.path().join("wt"), "t2").is_err());
    }
}
