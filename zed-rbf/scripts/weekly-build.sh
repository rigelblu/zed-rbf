#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
cd "$repo_root"

usage() {
  cat <<'USAGE'
Usage: zed-rbf/scripts/weekly-build.sh [options]

Build and install the current synced, conflict-free zed-rbf checkout.

Options:
  --debug                 Build and install the debug profile.
  --release               Build and install the release profile. Default.
  --install-dir <path>    Install directory. Default: $HOME/Applications.
  --name <name>           App bundle display name. Default: Zed RBF.
  --bundle-id <id>        App bundle identifier. Default: dev.zed.Zed-RBF.
  --log-path <path>       Weekly build log. Default: $HOME/Library/Logs/zed-rbf-weekly-build.log.
  --allow-dirty           Allow local working-copy changes.
  --check-only            Run preflight checks without building or installing.
  --open                  Open the installed app after installation.
  -h, --help              Show this help.
USAGE
}

fail() {
  echo "error: $*" >&2
  exit 1
}

require_value() {
  local option="$1"
  local value="${2:-}"
  if [[ -z "$value" || "$value" == --* ]]; then
    fail "$option requires a value"
  fi
}

release_build=true
install_dir="${HOME}/Applications"
app_name="Zed RBF"
bundle_id="dev.zed.Zed-RBF"
log_path="${HOME}/Library/Logs/zed-rbf-weekly-build.log"
allow_dirty=false
check_only=false
open_result=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug)
      release_build=false
      shift
      ;;
    --release)
      release_build=true
      shift
      ;;
    --install-dir)
      require_value "$1" "${2:-}"
      install_dir="$2"
      shift 2
      ;;
    --name)
      require_value "$1" "${2:-}"
      app_name="$2"
      shift 2
      ;;
    --bundle-id)
      require_value "$1" "${2:-}"
      bundle_id="$2"
      shift 2
      ;;
    --log-path)
      require_value "$1" "${2:-}"
      log_path="$2"
      shift 2
      ;;
    --allow-dirty)
      allow_dirty=true
      shift
      ;;
    --check-only)
      check_only=true
      shift
      ;;
    --open)
      open_result=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown option: $1"
      ;;
  esac
done

command -v jj >/dev/null 2>&1 || fail "jj not found"
command -v tee >/dev/null 2>&1 || fail "tee not found"
[[ -x "${repo_root}/zed-rbf/scripts/install-local.sh" ]] || fail "missing executable zed-rbf/scripts/install-local.sh"

mkdir -p "$(dirname "$log_path")"
touch "$log_path"

log_line() {
  printf '%s\n' "$*" | tee -a "$log_path"
}

log_block() {
  printf '%s\n' "$1" | tee -a "$log_path"
}

log_blank() {
  printf '\n' | tee -a "$log_path"
}

fail_after_log() {
  log_line "error: $*"
  exit 1
}

resolve_stack_tip() {
  local tip

  tip="$(jj log -r 'heads(@::)' --no-graph -T 'commit_id ++ "\n"')"
  if [[ -z "$tip" ]] || [[ "$(printf '%s\n' "$tip" | wc -l | tr -d ' ')" -ne 1 ]]; then
    fail_after_log "expected exactly one rbf stack tip above @; got: ${tip:-<none>}"
  fi

  stack_tip="$tip"
}

log_line "== zed-rbf weekly build =="
log_line "Started: $(date '+%Y-%m-%d %H:%M:%S %Z')"
log_line "Repo: $repo_root"
log_line "Log: $log_path"
log_blank

log_line "== jj provenance =="
log_line "Current revision:"
current_revision="$(jj log -r @ --no-graph --limit 1 2>&1)" || fail_after_log "could not read current jj revision"
log_block "$current_revision"
resolve_stack_tip
log_line "Rbf stack tip:"
stack_tip_revision="$(jj log -r "$stack_tip" --no-graph --limit 1 2>&1)" || fail_after_log "could not read rbf stack tip"
log_block "$stack_tip_revision"
main_revision="$(jj log -r 'present(main)' --no-graph --limit 1 2>/dev/null || true)"
if [[ -n "$main_revision" ]]; then
  log_line "Main revision:"
  log_block "$main_revision"
else
  log_line "Main revision: not present"
fi
log_blank

log_line "== preflight =="
status_output="$(jj diff --summary 2>&1)" || fail_after_log "could not read jj working-copy diff"
if [[ -n "$status_output" ]]; then
  log_block "$status_output"
else
  log_line "Working copy changes: none"
fi
if [[ "$allow_dirty" == "false" && -n "$status_output" ]]; then
  fail_after_log "working copy has changes; commit them first or pass --allow-dirty for dogfood"
fi
if [[ "$allow_dirty" == "true" && -n "$status_output" ]]; then
  log_line "Dirty working copy allowed by --allow-dirty."
fi

conflict_output="$(jj log -r "conflicts() & ::${stack_tip}" --no-graph --reversed 2>&1)" || fail_after_log "could not check jj conflicts"
if [[ -n "$conflict_output" ]]; then
  log_block "$conflict_output"
  fail_after_log "rbf stack has unresolved conflicts"
fi
log_line "No unresolved conflicts in the rbf stack."
log_blank

if [[ "$check_only" == "true" ]]; then
  log_line "Check-only mode complete; no build or install performed."
  exit 0
fi

install_args=(
  "${repo_root}/zed-rbf/scripts/install-local.sh"
  --install-dir "$install_dir"
  --name "$app_name"
  --bundle-id "$bundle_id"
)
if [[ "$release_build" == "false" ]]; then
  install_args+=(--debug)
fi
if [[ "$open_result" == "true" ]]; then
  install_args+=(--open)
fi

log_line "== install =="
printf 'Running:' | tee -a "$log_path"
printf ' %q' "${install_args[@]}" | tee -a "$log_path"
printf '\n' | tee -a "$log_path"
set +e
"${install_args[@]}" 2>&1 | tee -a "$log_path"
install_status=${PIPESTATUS[0]}
set -e
if [[ "$install_status" -ne 0 ]]; then
  fail_after_log "install-local failed"
fi
log_blank
log_line "Completed: $(date '+%Y-%m-%d %H:%M:%S %Z')"
