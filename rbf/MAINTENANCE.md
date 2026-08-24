---
title: Zed RB Flavour (RBF) Maintenance
---

# 🔵🌐 What is this
How zed-rbf evolves against upstream Zed: the manual sync flow, fork version handling, and the divergence inventory. For installing and using the fork, see [README.md](README.md).

# 🔵⋯ Sync From Upstream
```sh
rbf/scripts/upstream-sync.sh --check-only
rbf/scripts/upstream-sync.sh
```

- Remotes: `origin` = `rigelblu/zed-rbf` and `upstream` = `zed-industries/zed`.
- `--check-only` performs a real fetch, reports divergence/classification, and does not rebase.
- A real run fetches upstream, rebases the rbf stack onto `main@upstream`, and gates a clean rebase on `rbf/scripts/weekly-build.sh --check-only`, `cargo check -p zed`, the editor YMD test subset, and the recent-projects library tests.
- Conflicts are reported bottom-up with per-file classification; resolve lower stack entries first so fixes propagate to descendants.
- When upstream moved code we patched, accept upstream's new layout and port the small rbf hook into the new location.
- Undo rebase/local-history changes with the `jj op restore <op>` command printed and logged by the sync command. The command fetches first, so remote-tracking updates may remain.
- After resolving, use `rbf/scripts/weekly-build.sh` to build and install the current checkout as the local app.
- `--skip-verify` skips the post-rebase cargo/test gates, but it does not skip the weekly-build preflight.
- The `cargo check -p zed` gate requires the Metal Toolchain; if it is not installed, run `xcodebuild -downloadComponent MetalToolchain` or pass `--skip-verify` to skip the cargo/test gates.

# 🔵⋯ What A Rebase Breaks Silently
A clean `jj` stack and a green `cargo check` say almost nothing about whether the fork still works. The 2026-08 sync reached zero conflicts and a green workspace build with **eighteen** real defects still in it. Thirteen were found only by running the test suites.

Three failure classes, in the order they cost the most time:

- **Silent substitution.** Upstream rewrites a function the fork also changed. Both sides are valid, so there is no conflict to resolve and upstream's version simply wins. Every symbol still exists and every type still matches, so nothing fails to compile — the feature is present in the tree and unreachable at runtime. `#zed-37` lost four pieces this way: its `render_workspace_tabs` binding, its `.children(workspace_tabs)` element, its `NextProject`/`PreviousProject` handlers (repointed at the AI sidebar), and the entire body of `app_will_quit`.
- **API drift.** Fork code calls an upstream API that moved or was renamed — `RelPath::unix` → `from_unix_str`, `PathStyle::Posix` → `Unix`, `div_stack` entries wrapped in `DivStackEntry`, `LanguageConfig.matcher` becoming `Arc<LanguageMatcher>`. Half of these live in test code, which `cargo check` never compiles.
- **Signature widening.** The fork adds a parameter to a shared constructor; upstream later writes new call sites against the old arity. `#zed-24`'s `RecentProjectsDelegate::new(.., cx: &App)` will keep producing these every sync.

Triage habits that paid off:

- **A byte-identical fork file whose tests fail wholesale means lost wiring, not changed semantics.** `workspace_tabs.rs` matched the fork tip exactly while sixteen of its tests failed; the defect was three call sites in `multi_workspace.rs`.
- **Treat a discovered rename as tree-wide, not local.** Finding `PathStyle::Posix` → `Unix` in one file means grepping `crates/` for the rest.
- **Fix at the revision that owns the code**, not where the failure surfaced — otherwise the revisions between stay broken.
- **Keep the diff minimal.** Running `cargo fmt` on a whole file and squashing it low in the stack conflicted with every revision above it. Whole-file rewrites detonate a rebase stack.
- **Judge ports against the slice brief.** `#zed-16`'s brief settled single-file image paste, `#zed-37`'s settled the `retention_enabled` / `sidebar_ui_enabled` split, and `#zed-23`'s settled that finding #8 defers each commit's *diff*, never the list.

# 🔵⋯ Verification Gates
The gates in the sync flow above (`cargo check -p zed`, the editor YMD subset, recent-projects lib) are a preflight, not verification. They passed while all eighteen defects were live.

Run these before calling a sync done:

```sh
cargo test -p editor --lib
cargo test -p workspace --lib
cargo test -p git_ui --lib
cargo test -p project_panel --lib
cargo test -p recent_projects --lib
cargo test -p project --test integration
```

- Baseline at 2026-08-15: **1,891 passing, 0 failing.** A drop in the passing count is as meaningful as a failure.
- Run each crate separately. `cargo test` takes one filter, and a compile failure in one suite otherwise masks another's results.
- Do not use a name filter. `cargo test -p editor --lib ymd` ran 48 of 993 tests and reported green while fourteen image-paste tests were failing.
- `crates/project` sets `[lib] test = false`; its target is `--test integration`.
- `cargo check --workspace` does **not** compile `#[cfg(test)]` code. Four defects lived only there.

# 🔵⋯ Fork Version
`VERSION` lives in this directory. `crates/zed/build.rs` injects it at build time as `ZED_RBF_VERSION`.

To bump the visible fork version:
1. Edit `rbf/VERSION`.
2. Rebuild the app.
3. Verify with the bundled app binary, for example `"$HOME/Applications/Zed RBF.app/Contents/MacOS/zed-rbf" --system-specs`.

A missing or empty version file does not fail the build. It silently produces an unbranded app with no `(rbf v...)` window-title suffix and no `Zed RBF:` System Specs line, so always verify after touching it.

# 🔵⋯ Divergence Inventory
Use this during conflict triage to answer "is this file ours?" This inventory is curated from the current rebuilt stack; `rbf/scripts/upstream-sync.sh --check-only` supplements it with live merge-base classification.

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

Regenerate/check the list with `rbf/scripts/upstream-sync.sh --check-only`. It derives classification from the live upstream merge base, which also catches scaffold-level changes that a static setup-base diff misses.

Inventory-match semantics: every fork-touched path should either appear in the inventory above or be explainably covered by one of its grouped entries. The sync script produces the raw classification; curator judgment owns grouping it into this readable inventory.
