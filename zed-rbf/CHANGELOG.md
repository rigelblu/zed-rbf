---
title: "Zed RBF Changelog"
---

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
