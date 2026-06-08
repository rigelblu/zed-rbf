---
title: "Zed RBF Changelog"
---

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
