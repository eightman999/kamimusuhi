#!/usr/bin/env bash
# Keep the NAS mirror of the operator's Obsidian vault in step with GitHub.
# Runs on the Pi as the resident's service user.
#
#   obsidian-git.sh pull             fetch + fast-forward (never clobbers local work)
#   obsidian-git.sh commit <message> commit approved edits, rebase onto origin, push
#   obsidian-git.sh reset-to-remote  make the mirror exactly origin/<branch> (explicit)
#
# The token (GITHUB_PERSONAL_ACCESS_TOKEN) is handed to git through
# GIT_CONFIG_* environment variables only: it never appears in argv, in
# .git/config or in output.
set -euo pipefail
REPO="${OBSIDIAN_REPO:-/mnt/kamimusuhi/knowledge/obsidian}"
cmd="${1:?usage: $0 pull|commit <message>|reset-to-remote}"

if [[ -z "${GITHUB_PERSONAL_ACCESS_TOKEN:-}" && -r /srv/kamimusuhi/config/secrets.env ]]; then
  GITHUB_PERSONAL_ACCESS_TOKEN="$(sed -n 's/^GITHUB_PERSONAL_ACCESS_TOKEN=//p' /srv/kamimusuhi/config/secrets.env)"
fi
: "${GITHUB_PERSONAL_ACCESS_TOKEN:?GITHUB_PERSONAL_ACCESS_TOKEN is not set}"
basic="$(printf 'x-access-token:%s' "$GITHUB_PERSONAL_ACCESS_TOKEN" | base64 -w0)"
export GIT_CONFIG_COUNT=3
export GIT_CONFIG_KEY_0="http.https://github.com/.extraheader" GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $basic"
export GIT_CONFIG_KEY_1="safe.directory" GIT_CONFIG_VALUE_1="$REPO"
export GIT_CONFIG_KEY_2="credential.helper" GIT_CONFIG_VALUE_2=""
export GIT_TERMINAL_PROMPT=0
unset GITHUB_PERSONAL_ACCESS_TOKEN basic

[[ -d "$REPO/.git" ]] || { echo "not a git mirror: $REPO" >&2; exit 1; }
cd "$REPO"
branch="$(git rev-parse --abbrev-ref HEAD)"
dirty() { [[ -n "$(git status --porcelain)" ]]; }

case "$cmd" in
  pull)
    git fetch --quiet origin "$branch"
    if dirty; then
      echo "mirror has uncommitted changes; not merging (run reset-to-remote or commit)" >&2
      git status --short | head -5 >&2
      exit 2
    fi
    before="$(git rev-parse --short HEAD)"
    if ! git merge --ff-only --quiet "origin/$branch"; then
      echo "cannot fast-forward $before to origin/$branch (diverged)" >&2
      exit 3
    fi
    echo "pull ok: $before -> $(git rev-parse --short HEAD)"
    ;;
  commit)
    msg="${2:?commit message required}"
    git add -A
    if git diff --cached --quiet; then echo "nothing to commit"; exit 0; fi
    git -c user.name="Kamimusuhi (operator-approved)" \
        -c user.email="kamimusuhi@users.noreply.github.com" commit --quiet -m "$msg"
    git fetch --quiet origin "$branch"
    if ! git rebase --quiet "origin/$branch"; then
      git rebase --abort || true
      echo "rebase onto origin/$branch conflicted; commit kept locally, not pushed" >&2
      exit 3
    fi
    git push --quiet origin "HEAD:$branch"
    echo "pushed $(git rev-parse --short HEAD) to origin/$branch"
    ;;
  reset-to-remote)
    git fetch --quiet origin "$branch"
    git reset --quiet --hard "origin/$branch"
    git clean -qfd
    echo "mirror reset to $(git rev-parse --short HEAD) (origin/$branch)"
    ;;
  *) echo "unknown command: $cmd" >&2; exit 64 ;;
esac
