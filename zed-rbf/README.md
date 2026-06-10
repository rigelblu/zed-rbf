---
title: "Zed RBF"
---

# 🔵⋯ Context
This directory holds fork-owned Zed RBF files that should not be mixed into upstream Zed ownership.

# 🔵⋯ Prerequisites
- macOS with Xcode selected and the Metal Toolchain installed
- Rust tooling on `PATH`: `cargo` and `rustc`
- Network access on first install to fetch the pinned `cargo-bundle` helper and bundled `git`
- Enough build cache space for a Zed release or debug build

# 🔵⋯ Install The Local App
Run commands from the repository root. Build and install the current checkout as a local macOS app:

```sh
zed-rbf/scripts/install-local.sh
```

By default, the app is installed at `$HOME/Applications/Zed RBF.app` with bundle identifier `dev.zed.Zed-RBF`, so it can coexist with official Zed.

For temporary dogfood installs, use a separate install directory and app identity. `--debug` builds faster than release, but the result is larger and slower:
```sh
zed-rbf/scripts/install-local.sh \
  --debug \
  --install-dir /private/tmp/zed-rbf-install-local/Applications \
  --name "Zed RBF Dogfood" \
  --bundle-id dev.zed.Zed-RBF-Dogfood
```

Verify the default installed app identity from the bundled binary:
```sh
"$HOME/Applications/Zed RBF.app/Contents/MacOS/zed" --system-specs
```

For a custom `--install-dir` or `--name`, run `--system-specs` from that app bundle instead. The output should include a `Zed RBF: v...` line matching `zed-rbf/RBF_VERSION` and preserve the upstream `Zed: ...` line.

# 🔵⋯ FAQ
## 🟠⋯ Why Did The Installer Fail While Installing `cargo-bundle v0.6.1-zed`?
That error means the pinned Zed fork of `cargo-bundle` could not be installed or invoked from the build target's local tool cache.

Install the required version:
```sh
cargo_bundle_git_rev="2be2669972dff3ddd4daf89a2cb29d2d06cad7c7"
cargo install cargo-bundle \
  --git https://github.com/zed-industries/cargo-bundle.git \
  --rev "$cargo_bundle_git_rev" \
  --root "${CARGO_TARGET_DIR:-target}/tools/cargo-bundle-${cargo_bundle_git_rev}" \
  --force
```

Then rerun the installer:
```sh
zed-rbf/scripts/install-local.sh
```

Use a distinct bundle identifier when you want another side-by-side app identity:
```sh
zed-rbf/scripts/install-local.sh --name "Zed RBF Alt" --bundle-id dev.zed.Zed-RBF-Alt
```
