#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
cd "$repo_root"

# Cap how many files a single mirror may delete, as a backstop against a runaway
# --delete. Raise it for a legitimately large cleanup: RB_DRIVE_MAX_DELETE=500 ...
max_delete="${RB_DRIVE_MAX_DELETE:-100}"

usage() {
  cat <<'USAGE'
Usage: zed-rbf/scripts/sync-rb-drive.sh [--dry-run] <command>

Mirror the .rb-drive directory to a Google Drive destination with rsync.

Commands:
  remote      Back up .rb-drive into .rb-drive-remote/.rb-drive/ (faithful private mirror).
  public      Publish .rb-drive contents (.agents/, projects/) into .rb-drive-public-share/ (VCS + secrets excluded).

Options:
  -n, --dry-run  Show what rsync would change, without writing or deleting.
  -h, --help     Show this help.

Environment:
  RB_DRIVE_MAX_DELETE  Max files one mirror may delete (default: 100).
USAGE
}

fail() {
  echo "error: $*" >&2
  exit 1
}

dry_run=false
subcommand=""
for arg in "$@"; do
  case "$arg" in
    -n|--dry-run) dry_run=true ;;
    -h|--help) usage; exit 0 ;;
    remote|public)
      [ -z "$subcommand" ] || fail "only one command allowed (got '$subcommand' and '$arg')"
      subcommand="$arg"
      ;;
    *) fail "unknown argument: $arg" ;;
  esac
done

[ -n "$subcommand" ] || { usage >&2; exit 2; }

# A real .rb-drive store always contains projects/. Without this guard an empty or
# half-built source (e.g. a freshly re-created symlink before data syncs back) would
# make rsync --delete wipe the entire destination backup.
[ -d "${repo_root}/.rb-drive/projects" ] \
  || fail ".rb-drive/projects not found — refusing to mirror a missing or empty source with --delete"

# -L dereferences symlinks so the Drive copy holds real files; --delete mirrors the
# source exactly; --max-delete is the runaway-deletion backstop to the sentinel above.
# The array stays non-empty so "${rsync_opts[@]}" is safe under set -u on bash 3.2.
rsync_opts=(-avL --delete "--max-delete=${max_delete}")
if [ "$dry_run" = true ]; then
  rsync_opts+=(--dry-run)
fi

# Patterns withheld from the PUBLIC share only: VCS internals plus credential-shaped
# files, so a stray secret in .rb-drive can never ride along to a public Drive folder.
# This withholds only secrets, never documents — content is shared deliberately.
public_excludes=(
  --exclude='.git' --exclude='.jj'
  --exclude='.env*' --exclude='.netrc'
  --exclude='*.pem' --exclude='*.key' --exclude='*.p12' --exclude='*.pfx'
  --exclude='id_rsa*' --exclude='id_dsa*' --exclude='id_ecdsa*' --exclude='id_ed25519*'
  --exclude='*credential*' --exclude='*.keychain*'
)

case "$subcommand" in
  remote)
    # No trailing slash: rsync copies the .rb-drive directory itself, so the private
    # backup nests as <dest>/.rb-drive/ — a faithful, low-churn mirror of the store.
    dest="${repo_root}/.rb-drive-remote/"
    src="${repo_root}/.rb-drive"
    ;;
  public)
    # Trailing slash: rsync copies the CONTENTS, so .agents/ and projects/ land directly
    # at the share root (no .rb-drive/ level) for easy public browsing.
    rsync_opts+=("${public_excludes[@]}")
    dest="${repo_root}/.rb-drive-public-share/"
    src="${repo_root}/.rb-drive/"
    ;;
esac

# Require the destination to be a pre-created symlink (to the Drive folder); otherwise
# rsync would silently mirror into a local, gitignored dir that never reaches Drive.
[ -L "${dest%/}" ] || fail "${dest%/} is not a symlink — point it at your Drive folder before syncing"

rsync "${rsync_opts[@]}" "$src" "$dest"
