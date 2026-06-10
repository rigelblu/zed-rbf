#!/usr/bin/env bash

# Sync the zed-rbf stack onto upstream zed-industries/zed main with jj.
#
# Most conflicts land on upstream files carrying rbf hooks, but add/add and
# delete/rename edge cases still need human judgment. This command classifies
# and reports instead of auto-applying resolutions: port rbf hooks deliberately,
# inspect unexpected fork-owned collisions, then finish with zed-rbf/scripts/weekly-build.sh.

set -euo pipefail

check_only=false
skip_verify=false
log_path="$HOME/Library/Logs/zed-rbf-upstream-sync.log"

usage() {
  cat <<'USAGE'
Usage: zed-rbf/scripts/upstream-sync.sh [options]

Fetch upstream zed-industries/zed, rebase the rbf stack onto its main, and
report any conflicts with their divergence classification.

Options:
  --check-only        Fetch and report divergence and classification; no rebase.
  --skip-verify       Skip cargo check and regression tests after a clean rebase.
  --log-path <path>   Sync log. Default: $HOME/Library/Logs/zed-rbf-upstream-sync.log.
  -h, --help          Show this help.
USAGE
}

require_value() {
  local option="$1"
  local value="${2:-}"
  if [[ -z "$value" || "$value" == --* ]]; then
    echo "Error: $option requires a value." >&2
    exit 1
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check-only)
      check_only=true
      shift
      ;;
    --skip-verify)
      skip_verify=true
      shift
      ;;
    --log-path)
      require_value "$1" "${2:-}"
      log_path="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Error: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if ! command -v jj >/dev/null 2>&1; then
  echo "Error: jj (Jujutsu) is not installed or not in PATH." >&2
  exit 1
fi

repo_root="$(jj root 2>/dev/null)" || {
  echo "Error: not inside a jj repository." >&2
  exit 1
}
cd "$repo_root"

mkdir -p "$(dirname "$log_path")"

log() {
  printf '%s\n' "$*" | tee -a "$log_path"
}

run_logged() {
  "$@" 2>&1 | tee -a "$log_path"
}

upstream_ref="main@upstream"

resolve_stack_tip() {
  local tip

  tip="$(jj log -r 'heads(@::)' --no-graph -T 'commit_id ++ "\n"')"
  if [[ -z "$tip" ]] || [[ "$(printf '%s\n' "$tip" | wc -l | tr -d ' ')" -ne 1 ]]; then
    log "Error: expected exactly one rbf stack tip above @; got:"
    log "${tip:-<none>}"
    exit 1
  fi

  stack_tip="$tip"
}

log "== zed-rbf upstream sync =="
log "Started: $(date '+%Y-%m-%d %H:%M:%S %Z')"
log "Repo: $repo_root"
log "Log: $log_path"

log ""
log "== fetch =="
if ! run_logged jj git fetch --remote upstream; then
  log "Error: jj git fetch --remote upstream failed."
  exit 1
fi

if ! jj log -r "$upstream_ref" --no-graph --limit 1 -T '""' >/dev/null 2>&1; then
  log "Error: $upstream_ref not found. Configure the 'upstream' remote with a 'main' branch."
  exit 1
fi

resolve_stack_tip

fork_base="$(jj log -r "heads(::${stack_tip} & ::${upstream_ref})" --no-graph -T 'commit_id ++ "\n"')"
if [[ -z "$fork_base" ]] || [[ "$(printf '%s\n' "$fork_base" | wc -l | tr -d ' ')" -ne 1 ]]; then
  log "Error: expected exactly one merge base between the rbf stack tip and ${upstream_ref}; got:"
  log "${fork_base:-<none>}"
  exit 1
fi

upstream_new_count="$(jj log -r "${fork_base}..${upstream_ref}" --no-graph -T '"."' | wc -c | tr -d ' ')"
stack_count="$(jj log -r "${fork_base}..${stack_tip}" --no-graph -T '"."' | wc -c | tr -d ' ')"

log ""
log "== divergence =="
log "Merge base: $(jj log -r "$fork_base" --no-graph -T 'commit_id.short() ++ " " ++ description.first_line()')"
log "Upstream tip: $(jj log -r "$upstream_ref" --no-graph -T 'commit_id.short() ++ " " ++ description.first_line()')"
log "Rbf stack tip: $(jj log -r "$stack_tip" --no-graph -T 'commit_id.short() ++ " " ++ description.first_line()')"
log "New upstream commits since base: $upstream_new_count"
log "Rbf stack commits to transplant: $stack_count"

owned_files=""
hooked_files=""
deleted_files=""

parse_rename_or_copy_paths() {
  local display_path="$1"
  local prefix
  local renamed
  local suffix
  local old_middle
  local new_middle

  if [[ "$display_path" == *"{"* && "$display_path" == *" => "* && "$display_path" == *"}"* ]]; then
    prefix="${display_path%%\{*}"
    renamed="${display_path#*\{}"
    suffix="${renamed#*\}}"
    renamed="${renamed%%\}*}"
    old_middle="${renamed%% => *}"
    new_middle="${renamed#* => }"
    printf '%s\n%s\n' "${prefix}${old_middle}${suffix}" "${prefix}${new_middle}${suffix}"
    return 0
  fi

  if [[ "$display_path" == *" => "* ]]; then
    printf '%s\n%s\n' "${display_path%% => *}" "${display_path#* => }"
    return 0
  fi

  return 1
}

while IFS= read -r line; do
  [[ -z "$line" ]] && continue
  status="${line%% *}"
  rest="${line#* }"
  case "$status" in
    A)
      owned_files="${owned_files}${rest}"$'\n'
      ;;
    M)
      hooked_files="${hooked_files}${rest}"$'\n'
      ;;
    D)
      deleted_files="${deleted_files}${rest}"$'\n'
      ;;
    R)
      if paths="$(parse_rename_or_copy_paths "$rest")"; then
        old_path="$(printf '%s\n' "$paths" | sed -n '1p')"
        new_path="$(printf '%s\n' "$paths" | sed -n '2p')"
        owned_files="${owned_files}${new_path}"$'\n'
        deleted_files="${deleted_files}${old_path}"$'\n'
      else
        hooked_files="${hooked_files}${rest}"$'\n'
      fi
      ;;
    C)
      if paths="$(parse_rename_or_copy_paths "$rest")"; then
        new_path="$(printf '%s\n' "$paths" | sed -n '2p')"
        owned_files="${owned_files}${new_path}"$'\n'
      else
        owned_files="${owned_files}${rest}"$'\n'
      fi
      ;;
  esac
done < <(jj diff --from "$fork_base" --to "$stack_tip" --summary)

count_lines() {
  if [[ -z "$1" ]]; then
    echo 0
  else
    printf '%s' "$1" | grep -c '' | tr -d ' '
  fi
}

log ""
log "== classification =="
log "Fork-owned files (unexpected if conflicted): $(count_lines "$owned_files")"
log "Upstream files with rbf hooks (conflicts land here): $(count_lines "$hooked_files")"
printf '%s' "$hooked_files" | while IFS= read -r file; do
  [[ -n "$file" ]] && log "  hook: $file"
done
log "Deleted by fork: $(count_lines "$deleted_files")"

if [[ "$check_only" == true ]]; then
  log ""
  log "Check-only mode complete; no rebase performed."
  exit 0
fi

if [[ -n "$(jj diff --summary)" ]]; then
  log ""
  log "Error: working copy has changes; commit them before syncing."
  exit 1
fi

pre_op="$(jj op log --no-graph --limit 1 -T 'id.short(12)')"

log ""
log "== provenance =="
log "Pre-rebase operation: $pre_op (undo rebase/local-history changes with: jj op restore $pre_op)"
log "Pre-rebase stack tip: $stack_tip"
log "Pre-rebase change map:"
run_logged jj log -r "${fork_base}..${stack_tip}" --no-graph -T 'change_id.short() ++ " " ++ commit_id.short() ++ " " ++ description.first_line() ++ "\n"'

log ""
log "== rebase =="
if ! run_logged jj rebase -b @ -d "$upstream_ref"; then
  log "Error: jj rebase failed. Undo rebase/local-history changes with: jj op restore $pre_op"
  exit 1
fi

resolve_stack_tip
conflicted_revs="$(jj log -r "conflicts() & ${upstream_ref}..${stack_tip}" --no-graph --reversed -T 'change_id.short() ++ "\n"')"

if [[ -z "$conflicted_revs" ]]; then
  log "Rebase complete - no conflicts."
  if ! run_logged jj bookmark set main -r "$upstream_ref"; then
    log "Warning: could not move local main to $upstream_ref."
  fi

  log ""
  log "== verify =="
  if ! run_logged bash zed-rbf/scripts/weekly-build.sh --check-only --log-path "$log_path"; then
    log "Error: weekly-build preflight failed after rebase. Undo rebase/local-history changes with: jj op restore $pre_op"
    exit 1
  fi
  if [[ "$skip_verify" == true ]]; then
    log "Skipping cargo check and fork regression tests (--skip-verify)."
  else
    log "Running cargo check -p zed (use --skip-verify to skip)..."
    if ! run_logged cargo check -p zed; then
      log "Error: cargo check failed after rebase. Inspect, or undo rebase/local-history changes with: jj op restore $pre_op"
      exit 1
    fi
    log "Running cargo test -p editor ymd --lib..."
    if ! run_logged cargo test -p editor ymd --lib; then
      log "Error: editor YMD tests failed after rebase. Inspect, or undo rebase/local-history changes with: jj op restore $pre_op"
      exit 1
    fi
    log "Running cargo test -p recent_projects --lib..."
    if ! run_logged cargo test -p recent_projects --lib; then
      log "Error: recent_projects tests failed after rebase. Inspect, or undo rebase/local-history changes with: jj op restore $pre_op"
      exit 1
    fi
  fi

  log ""
  log "Sync complete. Install the weekly app with: zed-rbf/scripts/weekly-build.sh"
  exit 0
fi

log ""
log "== conflicts =="
log "Conflicted revisions (reported bottom-up so fixes propagate to descendants):"
printf '%s' "$conflicted_revs" | while IFS= read -r rev; do
  [[ -z "$rev" ]] && continue
  log ""
  log "$(jj log -r "$rev" --no-graph -T 'change_id.short() ++ " " ++ commit_id.short() ++ " " ++ description.first_line()')"
  while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    if printf '%s' "$hooked_files" | grep -Fxq "$file"; then
      log "  $file - rbf hook: accept upstream's new layout, port the hook (see zed-rbf/MAINTENANCE.md)"
    elif printf '%s' "$owned_files" | grep -Fxq "$file"; then
      log "  $file - UNEXPECTED: upstream collided with a fork-owned file; inspect manually"
    elif printf '%s' "$deleted_files" | grep -Fxq "$file"; then
      log "  $file - deleted or renamed by fork: decide whether to keep upstream's file or preserve the fork deletion/rename"
    else
      log "  $file - UNKNOWN: not in divergence inventory; inspect manually before choosing either side"
    fi
  done < <(jj log -r "$rev" --no-graph -T 'self.conflicted_files().map(|entry| entry.path() ++ "\n").join("")')
done

log ""
log "Resolve with: jj edit <rev>, fix the files, then continue up the stack."
log "Undo rebase/local-history changes with: jj op restore $pre_op"
log "After resolving all conflicts, rerun the gates: zed-rbf/scripts/weekly-build.sh --check-only && cargo check -p zed && cargo test -p editor ymd --lib && cargo test -p recent_projects --lib"
exit 1
