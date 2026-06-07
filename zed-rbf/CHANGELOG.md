---
title: "Zed RBF Changelog"
---

## 🟠⋯ v0.2.0 — #zed-02
- Added YMD syntax concealment in Markdown buffers: `==` highlight markers (and their color emoji) hide behind the highlight, with `editor::ToggleYmdConceal` revealing and re-concealing on demand
- Improved fold behavior around concealment: unfold-all (vim `zR`) reveals YMD syntax until the next toggle, local unfolds and fold UI ignore conceal folds, and saved folds never include them
- Fixed in the code-review pass before release: gutter chevrons count standard line-end folds correctly again, concealed indent-header rows stay foldable, fold-all in a concealed buffer folds instead of revealing, and runnable indicators survive concealed rows
- Fixed a crash when dragging a multi-line selection across concealed rows: a display point landing inside a conceal placeholder now clamps to the fold start instead of overshooting into a multibyte character (such as a concealed emoji), and character-drag selection clips to character boundaries like word/line drag already did
- Improved concealment performance: cursor motion reuses a buffer-version-keyed scan cache and refolds only the rows that changed instead of rescanning the whole buffer and rebuilding every conceal fold

## 🟠⋯ v0.1.0 — #zed-01
- Added YMD color styling in Markdown buffers: `==text==` renders as a background highlight, a leading circle emoji such as `==🔴text==` picks the highlight color, and a standalone circle emoji colors the whole line's foreground
- Supported circle emoji: 🔴 🟠 🟡 🟢 🔵 🟣 ⚫; `==text==` without an emoji uses the yellow default
