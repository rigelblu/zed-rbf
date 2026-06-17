---
title: Zed RB Flavour (RBF) Maintenance
---

# 🔵🌐 What is this
How zed-rbf evolves against upstream Zed: the manual sync flow, fork version handling, and the divergence inventory. For installing and using the fork, see [README.md](README.md).

# 🔵⋯ Sync From Upstream
```sh
zed-rbf/scripts/upstream-sync.sh --check-only
zed-rbf/scripts/upstream-sync.sh
```

- Remotes: `origin` = `rigelblu/zed-rbf` and `upstream` = `zed-industries/zed`.
- `--check-only` performs a real fetch, reports divergence/classification, and does not rebase.
- A real run fetches upstream, rebases the rbf stack onto `main@upstream`, and gates a clean rebase on `zed-rbf/scripts/weekly-build.sh --check-only`, `cargo check -p zed`, the editor YMD test subset, and the recent-projects library tests.
- Conflicts are reported bottom-up with per-file classification; resolve lower stack entries first so fixes propagate to descendants.
- When upstream moved code we patched, accept upstream's new layout and port the small rbf hook into the new location.
- Undo rebase/local-history changes with the `jj op restore <op>` command printed and logged by the sync command. The command fetches first, so remote-tracking updates may remain.
- After resolving, use `zed-rbf/scripts/weekly-build.sh` to build and install the current checkout as the local app.
- `--skip-verify` skips the post-rebase cargo/test gates, but it does not skip the weekly-build preflight.
- The `cargo check -p zed` gate requires the Metal Toolchain; if it is not installed, run `xcodebuild -downloadComponent MetalToolchain` or pass `--skip-verify` to skip the cargo/test gates.

# 🔵⋯ Fork Version
`RBF_VERSION` lives in this directory. `crates/zed/build.rs` injects it at build time as `ZED_RBF_VERSION`.

To bump the visible fork version:
1. Edit `zed-rbf/RBF_VERSION`.
2. Rebuild the app.
3. Verify with the bundled app binary, for example `"$HOME/Applications/Zed RBF.app/Contents/MacOS/zed" --system-specs`.

A missing or empty version file does not fail the build. It silently produces an unbranded app with no `(rbf v...)` window-title suffix and no `Zed RBF:` System Specs line, so always verify after touching it.

# 🔵⋯ Divergence Inventory
Use this during conflict triage to answer "is this file ours?" This inventory is curated from the current rebuilt stack; `zed-rbf/scripts/upstream-sync.sh --check-only` supplements it with live merge-base classification.

Compatibility wrappers outside this directory:
- `script/install-local` - delegates to `zed-rbf/scripts/install-local.sh`

Core fork hooks outside this directory:
- `crates/editor/src/ymd.rs` - YMD scanner and conceal engine
- `crates/project/src/markdown_table_formatter.rs` - table align on save

Upstream files carrying rbf hooks:
- `assets/keymaps/*` - Markdown shortcuts and vim `space c y`
- `assets/settings/default.json` - YMD defaults, Markdown defaults, and pinned projects
- `crates/paths` - Zed RBF path identity: regular Zed settings/extensions are shared while session data stays fork-specific
- `docs/src/reference/all-settings.md` - RBF settings documentation
- `crates/editor` - actions, clipboard, display map, editor settings, folds, hover, Markdown actions, selection
- `crates/git` and `crates/git_ui` - History, commit/ref comparison, file history, and file-scoped patch previews
- `crates/language` - Markdown language settings
- `crates/project` - table formatting, branch diff, project and LSP store hooks
- `crates/recent_projects` - pinned project management and picker rows
- `crates/release_channel`, `crates/settings`, `crates/settings_content`, `crates/system_specs`, `crates/workspace`, and `crates/zed` - fork version surfaces and settings integration

Regenerate/check the list with `zed-rbf/scripts/upstream-sync.sh --check-only`. It derives classification from the live upstream merge base, which also catches scaffold-level changes that a static setup-base diff misses.

Inventory-match semantics: every fork-touched path should either appear in the inventory above or be explainably covered by one of its grouped entries. The sync script produces the raw classification; curator judgment owns grouping it into this readable inventory.
