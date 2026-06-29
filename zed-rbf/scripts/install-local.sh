#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
cd "$repo_root"

usage() {
  cat <<'USAGE'
Usage: zed-rbf/scripts/install-local.sh [options]

Build and install the local zed-rbf macOS app bundle.

Options:
  --debug                 Build and bundle the debug profile.
  --release               Build and bundle the release profile. Default.
  --install-dir <path>    Install directory. Default: $HOME/Applications.
  --name <name>           App bundle display name. Default: Zed RBF.
  --bundle-id <id>        App bundle identifier. Default: dev.zed.Zed-RBF.
  --target <triple>       Rust target triple. Default: host triple.
  --generate-licenses     Accepted for compatibility; licenses are always regenerated.
  --open                  Open the installed app after installation.
  -h, --help              Show this help.
USAGE
}

fail() {
  echo "error: $*" >&2
  exit 1
}

sha256_file() {
  local file="$1"

  shasum -a 256 "$file" | awk '{print $1}'
}

require_value() {
  local option="$1"
  local value="${2:-}"
  if [[ -z "$value" || "$value" == --* ]]; then
    fail "$option requires a value"
  fi
}

set_plist_string() {
  local plist="$1"
  local key="$2"
  local value="$3"

  if /usr/libexec/PlistBuddy -c "Set :${key} ${value}" "$plist" 2>/dev/null; then
    return
  fi
  /usr/libexec/PlistBuddy -c "Add :${key} string ${value}" "$plist"
}

running_app_pids_for_executable() {
  local app_executable="$1"

  ps -axww -o pid= -o command= | awk -v app_executable="$app_executable" '
    {
      pid = $1
      sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", $0)
      if ($0 == app_executable || index($0, app_executable " ") == 1) {
        print pid
      }
    }
  '
}

request_app_quit() {
  local target_bundle_id="$1"

  /usr/bin/osascript - "$target_bundle_id" <<'APPLESCRIPT'
on run argv
  set targetBundleIdentifier to item 1 of argv
  tell application id targetBundleIdentifier to quit
end run
APPLESCRIPT
}

wait_for_app_exit() {
  local target_bundle_id="$1"
  local app_executable="$2"
  local timeout_seconds="$3"
  local deadline=$((SECONDS + timeout_seconds))
  local pids

  while true; do
    pids="$(running_app_pids_for_executable "$app_executable")"
    if [[ -z "$pids" ]]; then
      return
    fi
    if (( SECONDS >= deadline )); then
      fail "timed out waiting for app with bundle identifier $target_bundle_id to quit; still running pid(s): ${pids//$'\n'/, }"
    fi
    sleep 1
  done
}

quit_running_app_before_relaunch() {
  local target_bundle_id="$1"
  local app_executable="$2"
  local timeout_seconds="$3"
  local pids

  pids="$(running_app_pids_for_executable "$app_executable")"
  if [[ -z "$pids" ]]; then
    return
  fi

  echo "Requesting quit for running app with bundle identifier: $target_bundle_id"
  if ! request_app_quit "$target_bundle_id" >/dev/null; then
    fail "failed to request quit for running app with bundle identifier $target_bundle_id"
  fi
  echo "Waiting for running app to exit"
  wait_for_app_exit "$target_bundle_id" "$app_executable" "$timeout_seconds"
}

download_and_unpack() {
  local url="$1"
  local path_to_unpack="$2"
  local target_path="$3"
  local expected_sha256="$4"
  local temp_dir
  local archive_path
  local unpacked_path
  local actual_sha256

  command -v curl >/dev/null 2>&1 || fail "curl not found"
  command -v tar >/dev/null 2>&1 || fail "tar not found"
  command -v shasum >/dev/null 2>&1 || fail "shasum not found"

  temp_dir="$(mktemp -d)"
  archive_path="${temp_dir}/archive.tar.gz"
  if ! curl --silent --fail --location "$url" -o "$archive_path"; then
    rm -rf "$temp_dir"
    fail "failed to download $url"
  fi

  if ! tar -xzf "$archive_path" -C "$temp_dir" "$path_to_unpack"; then
    rm -rf "$temp_dir"
    fail "failed to unpack $path_to_unpack from $url"
  fi

  unpacked_path="${temp_dir}/${path_to_unpack#./}"
  if [[ ! -f "$unpacked_path" ]]; then
    rm -rf "$temp_dir"
    fail "archive did not contain $path_to_unpack"
  fi
  actual_sha256="$(sha256_file "$unpacked_path")"
  if [[ "$actual_sha256" != "$expected_sha256" ]]; then
    rm -rf "$temp_dir"
    fail "checksum mismatch for $path_to_unpack from $url: expected $expected_sha256, got $actual_sha256"
  fi
  if ! mkdir -p "$(dirname "$target_path")"; then
    rm -rf "$temp_dir"
    fail "failed to create target directory for $target_path"
  fi
  if ! mv "$unpacked_path" "$target_path"; then
    rm -rf "$temp_dir"
    fail "failed to move $path_to_unpack to $target_path"
  fi
  rm -rf "$temp_dir"
}

git_sha256_for_target() {
  local architecture="$1"

  case "$architecture" in
    aarch64-apple-darwin)
      printf '%s\n' "7d3b5018834ec1858a0d3f7fbfe2d3b7c7e6cec531f67edcac26a490d2809cac"
      ;;
    x86_64-apple-darwin)
      printf '%s\n' "e689226dbf0345432b8cdc6584f7c9e2780848a85b604e0faf6acf734ecb16c8"
      ;;
    *)
      fail "unsupported architecture for bundled git: $architecture"
      ;;
  esac
}

download_git() {
  local architecture="$1"
  local target_binary="$2"
  local git_version="v2.43.3"
  local git_version_sha="fa29823"
  local git_sha256

  git_sha256="$(git_sha256_for_target "$architecture")"
  case "$architecture" in
    aarch64-apple-darwin)
      download_and_unpack "https://github.com/desktop/dugite-native/releases/download/${git_version}/dugite-native-${git_version}-${git_version_sha}-macOS-arm64.tar.gz" ./bin/git "$target_binary" "$git_sha256"
      ;;
    x86_64-apple-darwin)
      download_and_unpack "https://github.com/desktop/dugite-native/releases/download/${git_version}/dugite-native-${git_version}-${git_version_sha}-macOS-x64.tar.gz" ./bin/git "$target_binary" "$git_sha256"
      ;;
  esac

  chmod +x "$target_binary"
}

app_name="Zed RBF"
bundle_id="dev.zed.Zed-RBF"
install_dir="${HOME}/Applications"
open_result=false
release_build=true
target_triple=""
target_requested=false
required_cargo_bundle_version="cargo-bundle v0.6.1-zed"
cargo_bundle_git_rev="2be2669972dff3ddd4daf89a2cb29d2d06cad7c7"
cargo_toml="${repo_root}/crates/zed/Cargo.toml"
cargo_toml_backup=""
install_backup=""
install_destination=""
install_temp_dir=""
install_completed=false
quit_timeout_seconds="${ZED_RBF_INSTALL_QUIT_TIMEOUT_SECONDS:-30}"

restore_cargo_toml() {
  if [[ -n "$cargo_toml_backup" && -f "$cargo_toml_backup" ]]; then
    cp "$cargo_toml_backup" "$cargo_toml"
    rm -f "$cargo_toml_backup"
  fi
}

restore_install_destination() {
  if [[ "$install_completed" != "true" && -n "$install_backup" && -e "$install_backup" ]]; then
    rm -rf "$install_destination"
    mv "$install_backup" "$install_destination"
  fi
}

cleanup_install_temp_dir() {
  if [[ -n "$install_temp_dir" && -d "$install_temp_dir" ]]; then
    rm -rf "$install_temp_dir"
  fi
}

cleanup() {
  restore_cargo_toml
  restore_install_destination
  cleanup_install_temp_dir
}

trap cleanup EXIT

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
    --target)
      require_value "$1" "${2:-}"
      target_triple="$2"
      target_requested=true
      shift 2
      ;;
    --generate-licenses)
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

[[ "$(uname -s)" == "Darwin" ]] || fail "zed-rbf/scripts/install-local.sh only supports macOS"
[[ -n "$app_name" ]] || fail "--name cannot be empty"
[[ "$app_name" != */* ]] || fail "--name cannot contain /"
[[ -n "$bundle_id" ]] || fail "--bundle-id cannot be empty"
[[ "$quit_timeout_seconds" =~ ^[0-9]+$ && "$quit_timeout_seconds" -gt 0 ]] || fail "ZED_RBF_INSTALL_QUIT_TIMEOUT_SECONDS must be a positive integer"
[[ -f "$cargo_toml" ]] || fail "missing $cargo_toml"
command -v cargo >/dev/null 2>&1 || fail "cargo not found"
command -v rustc >/dev/null 2>&1 || fail "rustc not found"
if [[ "$open_result" == "true" ]]; then
  command -v /usr/bin/osascript >/dev/null 2>&1 || fail "osascript not found"
fi
[[ -x /usr/libexec/PlistBuddy ]] || fail "PlistBuddy not found"

host_triple="$(rustc -vV | sed -n 's/^host: //p')"
[[ -n "$host_triple" ]] || fail "could not detect host target triple"
if [[ -z "$target_triple" ]]; then
  target_triple="$host_triple"
fi

case "$target_triple" in
  aarch64-apple-darwin|x86_64-apple-darwin)
    ;;
  *)
    fail "unsupported macOS target: $target_triple"
    ;;
esac

target_dir="release"
if [[ "$release_build" == "false" ]]; then
  target_dir="debug"
fi

target_root="${CARGO_TARGET_DIR:-target}"
case "$target_root" in
  /*) ;;
  *) target_root="${repo_root}/${target_root}" ;;
esac

build_output_dir="${target_root}/${target_dir}"
cargo_target_args=()
if [[ "$target_requested" == "true" ]]; then
  build_output_dir="${target_root}/${target_triple}/${target_dir}"
  cargo_target_args=(--target "$target_triple")
fi

channel="$(<"${repo_root}/crates/zed/RELEASE_CHANNEL")"
export ZED_RELEASE_CHANNEL="$channel"
export ZED_BUNDLE=true
export CXXFLAGS="-stdlib=libc++"

cargo_bundle_root="${target_root}/tools/cargo-bundle-${cargo_bundle_git_rev}"
cargo_bundle_bin="${cargo_bundle_root}/bin/cargo-bundle"
cargo_bundle_version=""
if [[ -x "$cargo_bundle_bin" ]]; then
  cargo_bundle_version="$("$cargo_bundle_bin" --help 2>&1 | head -n 1 || true)"
fi
if [[ "$cargo_bundle_version" != "$required_cargo_bundle_version" ]]; then
  rm -rf "$cargo_bundle_root"
  cargo install cargo-bundle \
    --git https://github.com/zed-industries/cargo-bundle.git \
    --rev "$cargo_bundle_git_rev" \
    --root "$cargo_bundle_root" \
    --force
fi
cargo_bundle_version="$("$cargo_bundle_bin" --help 2>&1 | head -n 1 || true)"
[[ "$cargo_bundle_version" == "$required_cargo_bundle_version" ]] || fail "failed to install $required_cargo_bundle_version at $cargo_bundle_bin"

echo "Generating bundled licenses"
script/generate-licenses

echo "Compiling zed and cli for $target_triple ($target_dir)"
cargo_build_args=(
  --package zed
  --package cli
  --features gpui_platform/runtime_shaders
)
if [[ "$target_requested" == "true" ]]; then
  cargo_build_args=("${cargo_target_args[@]}" "${cargo_build_args[@]}")
fi
if [[ "$release_build" == "true" ]]; then
  cargo_build_args=(--release "${cargo_build_args[@]}")
fi
cargo build "${cargo_build_args[@]}"

cargo_toml_backup="$(mktemp)"
cp "$cargo_toml" "$cargo_toml_backup"
sed -i.local-backup "s/package.metadata.bundle-${channel}/package.metadata.bundle/" "$cargo_toml"
rm -f "${cargo_toml}.local-backup"

echo "Creating application bundle"
pushd "${repo_root}/crates/zed" >/dev/null
cargo_bundle_args=(--select-workspace-root)
if [[ "$target_requested" == "true" ]]; then
  cargo_bundle_args=("${cargo_target_args[@]}" "${cargo_bundle_args[@]}")
fi
if [[ "$release_build" == "true" ]]; then
  cargo_bundle_args=(--release "${cargo_bundle_args[@]}")
fi
set +e
bundle_output="$(CARGO_BUNDLE_SKIP_BUILD=true "$cargo_bundle_bin" bundle "${cargo_bundle_args[@]}" 2>&1)"
bundle_status=$?
set -e
popd >/dev/null
printf '%s\n' "$bundle_output"
if [[ "$bundle_status" -ne 0 ]]; then
  fail "cargo bundle failed"
fi
restore_cargo_toml
cargo_toml_backup=""

app_path="$(printf '%s\n' "$bundle_output" | sed -n 's/^[[:space:]]*//; /[.]app$/p' | tail -n 1)"
[[ -n "$app_path" ]] || fail "could not parse app bundle path from cargo bundle output"
[[ -d "$app_path" ]] || fail "cargo bundle did not create an app bundle: $app_path"

app_parent="$(dirname "$app_path")"
renamed_app_path="${app_parent}/${app_name}.app"
if [[ "$app_path" != "$renamed_app_path" ]]; then
  rm -rf "$renamed_app_path"
  mv "$app_path" "$renamed_app_path"
  app_path="$renamed_app_path"
fi

plist="${app_path}/Contents/Info.plist"
[[ -f "$plist" ]] || fail "missing Info.plist: $plist"

set_plist_string "$plist" CFBundleName "$app_name"
set_plist_string "$plist" CFBundleDisplayName "$app_name"
set_plist_string "$plist" CFBundleIdentifier "$bundle_id"

document_icon_source="crates/zed/resources/Document.icns"
document_icon_target="${app_path}/Contents/Resources/Document.icns"
if [[ -f "$document_icon_source" ]]; then
  mkdir -p "$(dirname "$document_icon_target")"
  cp "$document_icon_source" "$document_icon_target"
else
  fail "missing document icon: $document_icon_source"
fi

cli_source="${build_output_dir}/cli"
cli_target="${app_path}/Contents/MacOS/cli"
[[ -f "$cli_source" ]] || fail "missing cli binary: $cli_source"
cp "$cli_source" "$cli_target"

git_cache="${build_output_dir}/git"
git_expected_sha256="$(git_sha256_for_target "$target_triple")"
if [[ -e "$git_cache" && ! -f "$git_cache" ]]; then
  fail "cached git path is not a file: $git_cache"
fi
if [[ -f "$git_cache" ]]; then
  git_actual_sha256="$(sha256_file "$git_cache")"
  if [[ "$git_actual_sha256" != "$git_expected_sha256" ]]; then
    echo "Cached git checksum mismatch; downloading a fresh binary for $target_triple"
    rm -f "$git_cache"
  fi
fi
if [[ ! -f "$git_cache" ]]; then
  echo "Downloading git binary for $target_triple"
  download_git "$target_triple" "$git_cache"
fi
chmod +x "$git_cache"
git_target="${app_path}/Contents/MacOS/git"
cp "$git_cache" "$git_target"

if command -v codesign >/dev/null 2>&1; then
  echo "Ad-hoc signing ${app_path}"
  codesign --force --deep --sign - "$app_path" >/dev/null
fi

mkdir -p "$install_dir"
install_destination="${install_dir}/${app_name}.app"
destination="$install_destination"
if [[ "$open_result" == "true" ]]; then
  quit_running_app_before_relaunch "$bundle_id" "${destination}/Contents/MacOS/zed" "$quit_timeout_seconds"
fi
install_temp_dir="$(mktemp -d "${install_dir}/.${app_name}.install.XXXXXX")"
temporary_destination="${install_temp_dir}/${app_name}.app"
install_backup="${install_temp_dir}/previous.app"
mv "$app_path" "$temporary_destination"
if [[ -e "$destination" ]]; then
  mv "$destination" "$install_backup"
fi
if ! mv "$temporary_destination" "$destination"; then
  restore_install_destination
  fail "failed to install application bundle to $destination"
fi
install_completed=true
cleanup_install_temp_dir
install_temp_dir=""
install_backup=""

echo "Installed application bundle: $destination"
echo "Bundle identifier: $bundle_id"

if [[ "$open_result" == "true" ]]; then
  open "$destination"
fi
