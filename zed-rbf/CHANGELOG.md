---
title: "Zed RBF Changelog"
---

## 🟠⋯ v0.24.0 — #zed-29
- Fixed Git Panel changed-file status refresh for file changes inside unloaded Project Panel directories without requiring directory expansion

## 🟠⋯ v0.23.0 — #zed-23
- Improved File History so it opens a file-scoped History view in the Git Panel instead of a separate graph tab
- File-scoped History lists only commits whose loaded diff includes the selected path, renders those rows without expand/collapse controls, and shows the full path in the scope-label tooltip
- File-scoped History now renders validated rows incrementally and self-refreshes during graph loading so the panel does not get stuck waiting for a tab switch
- File-scoped History keyboard navigation now updates one reusable file-diff preview instead of requiring a click or opening a tab for every selected commit
- Project Panel directory selections no longer open empty file-scoped History or fall back to the active editor; File History remains a file-only action
- Kept File History read-only: previewing active-file diffs does not checkout, restore, stage, or otherwise mutate the worktree

## 🟠⋯ v0.22.0 — #zed-22
- Added Git Panel Compare as a first-class read-only mode for reviewing current-workspace changes against a selected base without changing checkout
- Compare Since now opens Git Panel Compare with the selected commit as the base, so direct Compare and commit-based Compare share one workflow
- Improved Compare Since copy so commit patches, file patches, and current-workspace comparisons use distinct user-facing language
- Compare tabs and fixed-base labels now use short SHAs for raw commit bases, with tooltips that spell out the full current-workspace relationship
- Git Panel History commit and file row tooltips now use `Open Commit Diff` and `Open File Diff`

## 🟠⋯ v0.21.0 — #zed-21
- Added expandable Git Panel History rows that lazily show the files changed by a commit before opening the full commit diff
- File rows show added, modified, and deleted state and open a file-scoped commit diff without checking out the commit
- Expanded History files follow the shared Git Panel `Flat View` / `Tree View` mode, so nested commit files can be scanned as compact rows or directory-grouped rows from the existing panel menu
- History file expansion is a single-commit accordion, caches loaded file rows while the same repository remains active, and retries failed or canceled loads after collapse/re-expand
- History commit and file preview clicks keep focus on the History list, so Vim `j`/`k` and arrow keys continue walking commits and expanded rows; Vim `l` expands a commit, focuses its first visible expanded row, and opens a focused file row, while `h` returns to the commit row before collapsing it
- Expanded History rows reserve their measured height so file rows push later commits down instead of painting over them

## 🟠⋯ v0.20.0 — #zed-20
- Added Compare Since from commit diffs, opening a read-only workspace comparison against the selected commit without checking out that commit
- Kept exact commit comparisons separate from branch merge-base comparisons, so `Since` and `Merge` diff tabs with the same ref do not collide
- Compare Since tabs are fixed to their commit base; to compare from another commit, open that commit and use Compare Since again

## 🟠⋯ v0.19.0 — #zed-19
- Fixed Git Panel History in detached checkouts: when there is no active branch, History now loads commit rows from the current `HEAD` commit instead of staying stuck on the loading state
- Improved History state feedback: unscanned repositories stay in Loading, invalid `HEAD` state shows an error, and genuine empty history shows `No Commit History`
- Fixed SHA-based Git log sources to pass printable hex commit IDs to `git log`, which also repairs other SHA-backed log views that share `LogSource::Sha`

## 🟠⋯ v0.18.0 — #zed-18
- Added `markdown_image_paste_directory` for Markdown image paste, allowing pasted images to be saved under a configured file-relative directory instead of always using `assets`
- Invalid configured directories, including empty, absolute, parent-traversal, Markdown-unsafe, and percent-encoded values, fall back to `assets`
- Pasted-image writes reject existing symlinked target directories that resolve outside the target worktree before inserting the Markdown image link

## 🟠⋯ v0.17.0 — #zed-17
- Added clean display for Markdown image references with alt text: off the cursor row, `![alt](assets/image.png)` reads as underlined `alt` while cursor-line reveal shows the raw image syntax for editing
- Hovering a resolved local Markdown image reference shows the image in the standard hover popover; missing, remote, data, and unsupported paths fall back to normal hover behavior
- Empty-alt image links such as `![](assets/image.png)` stay raw until alt text exists, and Markdown buffers over the 100KB YMD cap skip this concealment and hover behavior

## 🟠⋯ v0.16.0 — #zed-16
- Added image-only clipboard paste for saved Markdown files: pasting an image writes bytes to an adjacent `.assets/` directory and inserts `![alt placeholder](.assets/<generated-name>.<ext>)`
- Text clipboard content continues to win over image content, non-Markdown or fileless buffers no-op, and multiple cursors receive the same generated link
- The editor writes the image before inserting Markdown, so failed writes leave the buffer unchanged and surface through the existing error notification path

## 🟠⋯ v0.15.0 — #zed-15
- Added Markdown formatting shortcuts in saved `.md` editor buffers: bold, italic, heading levels 1 through 6, bullets, and task lists with checked-state toggling
- Existing task rows now check and uncheck without leaving the whole row selected after the task-list shortcut runs
- Inline formatting wraps a selection or the word under the cursor, toggles existing markers off, inserts an empty marker pair when there is no word at the cursor, and leaves multi-line selections unchanged for row-level commands
- The shortcuts intentionally reuse common Markdown muscle memory (`Cmd` on macOS, `Ctrl` on Linux/Windows) while the `.md` editor is focused; palette actions still no-op outside Markdown language buffers

## 🟠⋯ v0.14.0 — #zed-14
- Fixed table alignment around section dividers: a contiguous pipe block that uses repeated delimiter rows as section breaks now aligns as one table while keeping later delimiter-looking rows as content
- Stacked tables with different delimiter column counts still split and align independently; no rows are dropped or duplicated
- Fixed clean YMD mode in Markdown tables: concealed color emoji, highlight markers, and inline style markers now disappear like normal YMD syntax while their hidden width is absorbed into trailing cell padding, so aligned tables keep the same visible columns with conceal on or off
- Added conceal support for balanced inline style markers (`**bold**`, `*italic*`, `_underline_`, and `~~strike~~`) while leaving common identifier text such as `snake_case` raw
- Inline style markers inside single- or multi-backtick code spans stay raw, so literal code examples do not hide `*`, `_`, or `~~` characters

## 🟠⋯ v0.13.0 — #zed-13
- Added Markdown table alignment on save: with `align_markdown_tables_on_save` enabled for Markdown, saving pads each table's columns to a consistent width so they line up, preserving the left, right, and center alignment markers (`:--`, `--:`, `:--:`)
- Cell widths are measured by display width, so a table whose cells hold wide glyphs or emoji still pads to aligned columns in the saved text
- Inline-code pipes inside CommonMark code spans, including multi-backtick spans, and escaped `\|` pipes are not treated as column boundaries; tables inside fenced code blocks are left exactly as written
- Opt-in and independent of `format_on_save`: alignment runs after your configured formatters whenever the setting is on, is a no-op outside Markdown buffers, and leaves an already-aligned table unchanged on a second save

## 🟠⋯ v0.12.0 — #zed-12
- Added a vim `space c y` binding that copies a workspace-relative code reference to the system clipboard: a cursor on line 42 copies `crates/editor/src/editor.rs:42`, and a visual selection of lines 42 through 58 copies `crates/editor/src/editor.rs:42-58`, ready to paste into an agent prompt, PR comment, or note
- Reuses the existing `editor::CopyFileLocation` action (still on the command palette); the binding just gives it the Neovim muscle-memory leader stroke, available whenever vim mode is on
- A linewise (`shift-v`) selection that stops at the start of the next row trims that trailing row, so selecting lines 42-44 copies `:42-44`, not `:42-45`; a characterwise selection that ends mid-line keeps its end row
- Fixed diff-editor line numbers: copying a file location from an expanded diff row now uses the underlying file line, not the visible diff row after deleted lines shift the hunk
- In vim normal and visual modes, plain `space` is now a pure leader (its old single-key right-motion is unbound), so `space c y` fires crisply with no pending-motion delay — matching the common Neovim space-as-leader convention
- The reference is relative to the active worktree and is written to the system clipboard, not a vim register, since the paste target is usually outside Zed; multi-worktree path ambiguity is not resolved here

## 🟠⋯ v0.11.0 — #zed-11
- Added Markdown task checkboxes as display characters: off the cursor row, `- [ ] task` reads as `□ task` and `- [x] task` reads as `■ task`, so task lists scan as checkboxes instead of raw markup
- Indentation is preserved, so a nested task keeps its indent and only the `- [ ] `/`- [x] ` marker is replaced; the task text after it is untouched
- Cursor-line reveal and `editor::ToggleYmdConceal` show the raw `- [ ]`/`- [x]` again for editing in place; the underlying text never changes
- Checkboxes inside fenced code blocks stay raw (the v0.9.0 code-fence exclusion), and changed task rows in an expanded diff hunk stay raw (the v0.8.0 diff-row contract)
- Configurable via `editor.ymd.checkbox_unchecked_char` and `editor.ymd.checkbox_checked_char`: any non-empty single-line string is used as-is (e.g. `"[x]"`); an empty or multi-line value falls back to the default `□`/`■`
- Dialect rules: only the dash bullet with a lowercase `x` is recognized — `* [ ]`, `+ [ ]`, and an uppercase `- [X]` stay raw for now

## 🟠⋯ v0.10.0 — #zed-10
- Added Markdown block-quote styling: a line that starts with a `>` marker (with up to three leading spaces) reads as quoted material — its content after the markers is muted and a vertical border is drawn in the gutter for the quote row
- Nested quotes count too: `>> nested` and the spaced `> > nested` both style, and a bare `>` line still gets the gutter border even though it has no content to mute
- The raw `>` markers stay visible — this version does not conceal them, so quotes read as quotes while remaining editable as Markdown
- YMD features inside a quote keep working: a `==highlight==`, color emoji, or `[label](url)` on a quote line keeps its own styling composed over the muted quote text
- Block quotes inside fenced code blocks stay fully raw (the v0.9.0 code-fence exclusion applies), so a `> ...` line in a code example is left literal
- Known constraints: the border sits in the gutter rather than at the content indent (a smaller first step than a painted content-column border), and continuation lines without an explicit `>` marker are not styled
- Diff rows stay raw as elsewhere: a changed quote line in an expanded diff hunk shows no muting or border

## 🟠⋯ v0.9.0 — #zed-09
- Added code-fence exclusion: fenced code blocks (``` or ~~~) keep their contents fully literal — YMD never colors, conceals, underlines, or rules anything inside a fence, so pasted code with YMD-like syntax (`==text==`, color emoji, `[label](url)`, `---`) stays exactly as written
- The whole fenced block including its delimiter lines is excluded, and an unclosed fence keeps the rest of the file literal so in-progress code stays raw as you type; ordinary Markdown outside fences still styles normally
- A `---` inside a fence no longer renders as a horizontal rule (closing the deferral noted in v0.7.0); indented code blocks and inline code spans stay in scope for a later slice

## 🟠⋯ v0.8.0 — #zed-08
- Added always-raw YMD syntax in expanded diff hunks: expanding a Markdown diff hunk shows every changed row's exact syntax — `==` highlight markers, color emoji, link `[label](url)`, and `---` dashes — with no concealment, no color, and no rule blocks, on both the added and the deleted side, so diff review shows precisely what changed
- Expanding or collapsing a hunk updates the reveal immediately, and collapsing restores the clean concealed view (with color and rule blocks) on those rows
- Normal non-diff rows are unaffected: clean concealed display off the cursor row, with cursor-line reveal and `editor::ToggleYmdConceal` unchanged

## 🟠⋯ v0.7.0 — #zed-07
- Added Markdown horizontal rules: a thematic-break line (`---`, `----`, or spaced `- - -`) renders as a thin full-width hairline rule, so section breaks read as structure instead of raw dashes
- The rule is a quiet 1px hairline in the theme's border color, vertically centered in the row — a calm separator, not a heavy bar
- Cursor-line reveal and `editor::ToggleYmdConceal` show the raw `---` for editing in place; the underlying text never changes, so yanking a rule row copies the raw `---`
- Known constraint: because a rule reveals on its cursor row, deleting the line directly above one briefly shows its raw `---` as the cursor lands there — the dashes are never lost, and the hairline returns the moment the cursor leaves the row
- Improved performance: rule rendering reuses the buffer-version-keyed YMD scan cache shared with concealment, so moving the cursor never rescans the buffer on a rule-bearing file
- Only the dash family renders: `***` and `___` thematic breaks deliberately stay raw, and a document's top-of-file YAML frontmatter delimiters are left alone
- Thematic breaks inside fenced code blocks and changed `---` rows in expanded diff hunks may still render as rules until later slices add code-fence and diff-row exclusion

## 🟠⋯ v0.6.0 — #zed-06
- Added inline Markdown link concealment: a single-line `[label](url)` reads as just its `label` in clean view, with the `[` and `](url)` syntax hidden so links scan like links, not raw markup
- The label carries a quiet 1px underline in the default text color — calm and professional, deliberately not a loud link-blue — so a link inside a `==highlight==` keeps the highlight's colors and only gains the underline
- URLs with balanced parentheses fold whole, so `[Rust (film)](https://en.wikipedia.org/wiki/Rust_(film))` reads as `Rust (film)` with no stray `)` left behind
- Cursor-line reveal and `editor::ToggleYmdConceal` show the raw `[label](url)` again; images `![alt](src)`, empty-URL `[x]()`, and unterminated `[x](url` links stay raw
- This version trades read-mode link-clicking for clean reading; cursor-row reveal restores the clickable URL until a dedicated cmd-click slice lands

## 🟠⋯ v0.5.0 — #zed-05
- Added emoji heading concealment: a Markdown heading whose text carries a YMD color emoji hides its leading `#` run in clean view, so `# 🔵 Blue Heading` reads as `Blue Heading`; plain headings keep their `#`
- When the color emoji sits right after the `#` prefix, the emoji and its trailing space fold away too, so the revealed heading starts with its first word — a `## 🟠⋯ Heading` keeps its visible `⋯` and reads as `⋯ Heading`
- Cursor-line reveal and `editor::ToggleYmdConceal` show the raw `#` prefix again, and concealment never touches buffer text, so the outline panel and breadcrumbs still navigate by full heading text

## 🟠⋯ v0.4.0 — #zed-04
- Added standalone line-color emoji concealment: the first effective color emoji on a line (its marker) hides in clean view while the line keeps its color; cursor-line reveal brings it back
- One marker per line: later color emojis are content and stay visible

## 🟠⋯ v0.3.0 — #zed-03
- Added cursor-line reveal: the line under each cursor head shows its raw YMD markers while every other line stays clean, re-concealing as the cursor leaves
- Same-row cursor motion does zero fold work; multi-cursor reveals each head row
- Fixed a dead `editor::ToggleYmdConceal` press: when the cursor already revealed every marker row, the toggle now re-conceals reliably instead of doing nothing because the resync distinguishes "no folds because revealed" from "no folds because leaked"
- Improved large-file handling: Markdown buffers over the 100KB cap skip all conceal work on cursor motion via an O(1) size check, instead of re-running snapshots and a whole-buffer fold scan on every selection change

## 🟠⋯ v0.2.0 — #zed-02
- Added YMD syntax concealment in Markdown buffers: `==` highlight markers (and their color emoji) hide behind the highlight, with `editor::ToggleYmdConceal` revealing and re-concealing on demand
- Improved fold behavior around concealment: unfold-all (vim `zR`) reveals YMD syntax until the next toggle, local unfolds and fold UI ignore conceal folds, and saved folds never include them
- Fixed in the code-review pass before release: gutter chevrons count standard line-end folds correctly again, concealed indent-header rows stay foldable, fold-all in a concealed buffer folds instead of revealing, and runnable indicators survive concealed rows
- Fixed a crash when dragging a multi-line selection across concealed rows: a display point landing inside a conceal placeholder now clamps to the fold start instead of overshooting into a multibyte character (such as a concealed emoji), and character-drag selection clips to character boundaries like word/line drag already did
- Improved concealment performance: cursor motion reuses a buffer-version-keyed scan cache and refolds only the rows that changed instead of rescanning the whole buffer and rebuilding every conceal fold

## 🟠⋯ v0.1.0 — #zed-01
- Added YMD color styling in Markdown buffers: `==text==` renders as a background highlight, a leading circle emoji such as `==🔴text==` picks the highlight color, and a standalone circle emoji colors the whole line's foreground
- Supported circle emoji: 🔴 🟠 🟡 🟢 🔵 🟣 ⚫; `==text==` without an emoji uses the yellow default
