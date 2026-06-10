---
title: Zed RB Flavour (RBF) Maintenance
---

# 🔵🌐 What is this
How zed-rbf evolves against upstream Zed: the manual sync flow, fork version handling, and the divergence inventory. For installing and using the fork, see [README.md](README.md).

# 🔵⋯ Sync From Upstream
There is no fork-owned upstream-sync script in this slice. The flow is manual until `#zed-28` adds automation:

- Remotes: `origin` = `rigelblu/zed-rbf` and `upstream` = `zed-industries/zed`.
- Fetch both remotes with `jj git fetch`.
- Rebase the rbf stack onto the new upstream `main`.
- Resolve conflicts bottom-up.
- When upstream moved code we patched, accept upstream's new layout and port the small rbf hook into the new location.
- After resolving, use `zed-rbf/scripts/install-local.sh` to build and install the current checkout as the local app.

# 🔵⋯ Fork Version
`RBF_VERSION` lives in this directory. `crates/zed/build.rs` injects it at build time as `ZED_RBF_VERSION`.

To bump the visible fork version:
1. Edit `zed-rbf/RBF_VERSION`.
2. Rebuild the app.
3. Verify with the bundled app binary, for example `"$HOME/Applications/Zed RBF.app/Contents/MacOS/zed" --system-specs`.

A missing or empty version file does not fail the build. It silently produces an unbranded app with no `(rbf v...)` window-title suffix and no `Zed RBF:` System Specs line, so always verify after touching it.

# 🔵⋯ Divergence Inventory
Use this during conflict triage to answer "is this file ours?" This inventory is curated from the current rebuilt stack; `#zed-28` replaces this manual inventory with sync-script output.

Compatibility wrappers outside this directory:
- `script/install-local` - delegates to `zed-rbf/scripts/install-local.sh`

Core fork hooks outside this directory:
- `crates/editor/src/ymd.rs` - YMD scanner and conceal engine
- `crates/project/src/markdown_table_formatter.rs` - table align on save

Upstream files carrying rbf hooks:
- `assets/keymaps/*` - Markdown shortcuts and vim `space c y`
- `assets/settings/default.json` - YMD defaults, Markdown defaults, and pinned projects
- `docs/src/reference/all-settings.md` - RBF settings documentation
- `crates/editor` - actions, clipboard, display map, editor settings, folds, hover, Markdown actions, selection
- `crates/git` and `crates/git_ui` - History, commit/ref comparison, file history, and file-scoped patch previews
- `crates/language` - Markdown language settings
- `crates/project` - table formatting, branch diff, project and LSP store hooks
- `crates/recent_projects` - pinned project management and picker rows
- `crates/release_channel`, `crates/settings`, `crates/settings_content`, `crates/system_specs`, `crates/workspace`, and `crates/zed` - fork version surfaces and settings integration

Inventory-match semantics: every fork-touched path should either appear in the inventory above or be explainably covered by one of its grouped entries. Curator judgment owns the grouping until `#zed-28` adds generated classification output.
