use gpui::{HighlightStyle, Hsla, UnderlineStyle, px, rgba};
use std::ops::Range;
use theme::Appearance;

pub(crate) const MAX_YMD_HIGHLIGHT_BYTES: usize = 100_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum YmdColor {
    Default,
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Black,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum YmdHighlightKind {
    Background(YmdColor),
    LineForeground(YmdColor),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct YmdHighlight {
    pub range: Range<usize>,
    pub kind: YmdHighlightKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct YmdConceal {
    pub range: Range<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct YmdLink {
    pub open_marker_range: Range<usize>,
    pub label_range: Range<usize>,
    pub close_marker_range: Range<usize>,
}

impl YmdLink {
    // The URL byte range inside the `](url)` close marker — the destination
    // bytes after the leading `](` and before the trailing `)`. The markers are
    // always single ASCII bytes (`]`, `(`, `)`), so this stays on char
    // boundaries. Used by the cmd-click hover path (#zed-35) to recover the
    // concealed URL the conceal fold hides from the display.
    pub(crate) fn url_range(&self) -> Range<usize> {
        self.close_marker_range.start + 2..self.close_marker_range.end - 1
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct YmdImage {
    pub open_marker_range: Range<usize>,
    pub alt_text_range: Range<usize>,
    pub close_marker_range: Range<usize>,
    pub path_range: Range<usize>,
    pub reference_range: Range<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct YmdBlockQuote {
    // The whole quote line (no trailing newline); the editor paints a vertical
    // gutter border over its rows.
    pub line_range: Range<usize>,
    // The quote text after the `>` markers; muted by the highlight pass. Empty
    // for a bare `>` line, which still gets the border but no text to mute.
    pub content_range: Range<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct YmdCheckbox {
    // The `- [ ] ` / `- [x] ` marker bytes (list bullet through the trailing
    // space), which the editor replaces with a single display character. Any
    // leading indentation stays visible, so a nested task keeps its indent.
    pub range: Range<usize>,
    pub checked: bool,
}

struct YmdInlineHighlight {
    range: Range<usize>,
    open_marker_range: Range<usize>,
    close_marker_range: Range<usize>,
    color: YmdColor,
}

impl YmdColor {
    pub const ALL: [Self; 8] = [
        Self::Default,
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Purple,
        Self::Black,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::Default => 0,
            Self::Red => 1,
            Self::Orange => 2,
            Self::Yellow => 3,
            Self::Green => 4,
            Self::Blue => 5,
            Self::Purple => 6,
            Self::Black => 7,
        }
    }

    fn from_emoji(text: &str) -> Option<(Self, usize)> {
        [
            ("🔴", Self::Red),
            ("🟠", Self::Orange),
            ("🟡", Self::Yellow),
            ("🟢", Self::Green),
            ("🔵", Self::Blue),
            ("🟣", Self::Purple),
            ("⚫", Self::Black),
        ]
        .into_iter()
        .find_map(|(emoji, color)| text.starts_with(emoji).then_some((color, emoji.len())))
    }

    fn first_match_in(text: &str) -> Option<(usize, Self, usize)> {
        [
            ("🔴", Self::Red),
            ("🟠", Self::Orange),
            ("🟡", Self::Yellow),
            ("🟢", Self::Green),
            ("🔵", Self::Blue),
            ("🟣", Self::Purple),
            ("⚫", Self::Black),
        ]
        .into_iter()
        .filter_map(|(emoji, color)| text.find(emoji).map(|index| (index, color, emoji.len())))
        .min_by_key(|(index, _, _)| *index)
    }

    fn background(self, appearance: Appearance) -> Hsla {
        let color = match (self, appearance) {
            (Self::Default | Self::Yellow, Appearance::Light) => rgba(0xfff59dff),
            (Self::Default | Self::Yellow, Appearance::Dark) => rgba(0x5c5637ff),
            (Self::Red, Appearance::Light) => rgba(0xffcdd2ff),
            (Self::Red, Appearance::Dark) => rgba(0x5c3a3aff),
            (Self::Orange, Appearance::Light) => rgba(0xffe0b2ff),
            (Self::Orange, Appearance::Dark) => rgba(0x5c4a37ff),
            (Self::Green, Appearance::Light) => rgba(0xc8e6c9ff),
            (Self::Green, Appearance::Dark) => rgba(0x3a5c3aff),
            (Self::Blue, Appearance::Light) => rgba(0xbbdefbff),
            (Self::Blue, Appearance::Dark) => rgba(0x3a4a5cff),
            (Self::Purple, Appearance::Light) => rgba(0xe1bee7ff),
            (Self::Purple, Appearance::Dark) => rgba(0x4a3a5cff),
            (Self::Black, Appearance::Light) => rgba(0xe0e0e0ff),
            (Self::Black, Appearance::Dark) => rgba(0x3a3a3aff),
        };
        color.into()
    }

    fn line_foreground(self, appearance: Appearance) -> Hsla {
        let color = match (self, appearance) {
            (Self::Default, Appearance::Light) => rgba(0x1a1a1aff),
            (Self::Default, Appearance::Dark) => rgba(0xe0def4ff),
            (Self::Red, Appearance::Light) => rgba(0xb71c1cff),
            (Self::Red, Appearance::Dark) => rgba(0xffcdd2ff),
            (Self::Orange, Appearance::Light) => rgba(0xe65100ff),
            (Self::Orange, Appearance::Dark) => rgba(0xffe0b2ff),
            (Self::Yellow, Appearance::Light) => rgba(0xf57f17ff),
            (Self::Yellow, Appearance::Dark) => rgba(0xfff59dff),
            (Self::Green, Appearance::Light) => rgba(0x1b5e20ff),
            (Self::Green, Appearance::Dark) => rgba(0xc8e6c9ff),
            (Self::Blue, Appearance::Light) => rgba(0x0d47a1ff),
            (Self::Blue, Appearance::Dark) => rgba(0xbbdefbff),
            (Self::Purple, Appearance::Light) => rgba(0x4a148cff),
            (Self::Purple, Appearance::Dark) => rgba(0xe1bee7ff),
            (Self::Black, Appearance::Light) => rgba(0x212121ff),
            (Self::Black, Appearance::Dark) => rgba(0xe0e0e0ff),
        };
        color.into()
    }
}

pub(crate) fn background_style(color: YmdColor, appearance: Appearance) -> HighlightStyle {
    HighlightStyle {
        color: Some(match appearance {
            Appearance::Light => rgba(0x1a1a1aff).into(),
            Appearance::Dark => rgba(0xe0def4ff).into(),
        }),
        background_color: Some(color.background(appearance)),
        ..Default::default()
    }
}

pub(crate) fn line_foreground_style(color: YmdColor, appearance: Appearance) -> HighlightStyle {
    HighlightStyle {
        color: Some(color.line_foreground(appearance)),
        ..Default::default()
    }
}

// A 1px underline that INHERITS the label text color (no color override), so it
// always matches the blue Zed's Markdown grammar (`@link_text.markup`) paints the
// label — the same blue the raw `[label](url)` shows when revealed — instead of a
// separate pinned color. Matching the text reads as one coherent link in every
// theme; an earlier revision (#zed-06 Q4) pinned this to the YMD default
// foreground for calm, but that left blue text under a near-black underline
// (#zed-35 dogfood, 2026-06-20). The style sets only the underline, so a label
// inside a `==…==` highlight keeps that highlight's color and gains this line.
pub(crate) fn link_style() -> HighlightStyle {
    HighlightStyle {
        underline: Some(UnderlineStyle {
            thickness: px(1.),
            color: None,
            ..UnderlineStyle::default()
        }),
        ..HighlightStyle::default()
    }
}

// Mutes the quote content to read as quoted material. The color is the theme's
// muted text color, resolved by the editor (the gutter border supplies the
// vertical separator). Only the foreground is set, so a YMD highlight inside the
// quote keeps its own background and overrides this foreground on overlap (the
// `YmdBackground`-after-`YmdLineForeground` ordering rule extends here).
pub(crate) fn block_quote_style(color: Hsla) -> HighlightStyle {
    HighlightStyle {
        color: Some(color),
        ..HighlightStyle::default()
    }
}

fn for_each_line(text: &str, mut callback: impl FnMut(&str, usize)) {
    let mut line_start = 0;

    for line in text.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let line_content_end = line_end - usize::from(line.ends_with('\n'));
        callback(&text[line_start..line_content_end], line_start);
        line_start = line_end;
    }
}

// Byte offset past the YAML frontmatter block, or 0 when there is none. Walk Q3 —
// KEEP LOOSE (Tom's call): ANY break-shaped line 1 opens the skip, not only a bare
// `---`. The block ends after the next break-shaped line (its closing delimiter);
// with no closing delimiter only line 1 is skipped. The deliberate consequence: a
// frontmatter-less document that opens with a real rule loses that rule's
// replacement, and — because the following break-shaped line is then read as the
// closing delimiter — the next rule's replacement too. Documented as a bet in the
// brief; no tightening here.
fn frontmatter_skip_end(text: &str) -> usize {
    let Some(first_line) = text.split_inclusive('\n').next() else {
        return 0;
    };
    let first_line_content_end = first_line.len() - usize::from(first_line.ends_with('\n'));
    if !is_thematic_break_line(&text[..first_line_content_end]) {
        return 0;
    }

    let mut line_start = first_line.len();
    for line in text[line_start..].split_inclusive('\n') {
        let line_end = line_start + line.len();
        let line_content_end = line_end - usize::from(line.ends_with('\n'));
        if is_thematic_break_line(&text[line_start..line_content_end]) {
            return line_end;
        }
        line_start = line_end;
    }

    first_line.len()
}

// A dash-only Markdown thematic break: up to 3 leading spaces, then a run of `-`
// (>= 3 total) with optional interior spaces/tabs (so `- - -` qualifies). Walk Q2:
// the `*`/`_` families are deliberately NOT recognized — only `-` renders as a
// rule. Indented 4+ spaces (code) and any other character disqualify the line.
fn is_thematic_break_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    let leading_spaces = bytes.iter().take_while(|byte| **byte == b' ').count();
    if leading_spaces > 3 {
        return false;
    }

    let mut dash_count = 0;
    for byte in &bytes[leading_spaces..] {
        match *byte {
            b'-' => dash_count += 1,
            b' ' | b'\t' => {}
            _ => return false,
        }
    }

    dash_count >= 3
}

pub(crate) fn scan(text: &str) -> Vec<YmdHighlight> {
    let mut highlights = Vec::new();
    let fenced_code_ranges = fenced_code_block_ranges(text);

    for_each_line(text, |line_content, line_start| {
        if range_overlaps_any(
            &(line_start..line_start + line_content.len()),
            &fenced_code_ranges,
        ) {
            return;
        }
        let inline_highlights = scan_line_background_markups(line_content, line_start);
        for inline_highlight in &inline_highlights {
            highlights.push(YmdHighlight {
                range: inline_highlight.range.clone(),
                kind: YmdHighlightKind::Background(inline_highlight.color),
            });
        }

        let captures = capture_ranges(&inline_highlights, line_start);
        if let Some((_, color)) = first_effective_emoji(line_content, &captures) {
            highlights.push(YmdHighlight {
                range: line_start..line_start + line_content.len(),
                kind: YmdHighlightKind::LineForeground(color),
            });
        }
    });

    highlights
}

pub(crate) fn scan_links(text: &str) -> Vec<YmdLink> {
    let mut links = Vec::new();
    let fenced_code_ranges = fenced_code_block_ranges(text);
    for_each_line(text, |line_content, line_start| {
        if range_overlaps_any(
            &(line_start..line_start + line_content.len()),
            &fenced_code_ranges,
        ) {
            return;
        }
        links.extend(scan_line_links(line_content, line_start));
    });
    links
}

// Resolve the concealed URL of the Markdown link whose visible label contains
// `offset` (an absolute byte offset into `text`). Returns the label's byte range
// — the hover underline target — and the URL string, or `None` when `offset` is
// not inside any concealed link label (it is in the brackets, the URL, plain
// text, or a fenced code block). Backs the cmd-click hover path (#zed-35): the
// buffer still holds the full `[label](url)` because conceal is display-only, so
// a click on the visible label can open the hidden URL. Reusing `scan_links`
// keeps the clickable label set identical to the concealed label set (same
// fenced-code exclusion), so reading and clicking never disagree.
pub(crate) fn markdown_link_url_at_offset(
    text: &str,
    offset: usize,
) -> Option<(Range<usize>, String)> {
    scan_links(text).into_iter().find_map(|link| {
        if !link.label_range.contains(&offset) {
            return None;
        }
        let url = text.get(link.url_range())?.to_string();
        Some((link.label_range, url))
    })
}

pub(crate) fn scan_images(text: &str) -> Vec<YmdImage> {
    let mut images = Vec::new();
    let fenced_code_ranges = fenced_code_block_ranges(text);
    for_each_line(text, |line_content, line_start| {
        if range_overlaps_any(
            &(line_start..line_start + line_content.len()),
            &fenced_code_ranges,
        ) {
            return;
        }
        images.extend(scan_line_images(line_content, line_start));
    });
    images
}

// Byte ranges of Markdown thematic-break lines (`---`, `----`, spaced `- - -`)
// that should render as a horizontal rule. Permanent dialect rule (walk Q2): only
// the dash (`-`) family is recognized — `***` and `___` thematic breaks stay raw.
// A leading run of break-shaped lines is skipped as YAML frontmatter delimiters
// (see `frontmatter_skip_end`); the loose opener is a deliberate bet documented in
// the brief. Each returned range covers the line's content (no trailing newline),
// which the editor replaces with a `BlockPlacement::Replace` rule block.
pub(crate) fn scan_thematic_breaks(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let frontmatter_skip_end = frontmatter_skip_end(text);
    let fenced_code_ranges = fenced_code_block_ranges(text);

    for_each_line(text, |line_content, line_start| {
        if range_overlaps_any(
            &(line_start..line_start + line_content.len()),
            &fenced_code_ranges,
        ) {
            return;
        }
        if line_start >= frontmatter_skip_end && is_thematic_break_line(line_content) {
            ranges.push(line_start..line_start + line_content.len());
        }
    });

    ranges
}

// Markdown block-quote lines (`>` with up to three leading spaces, nesting via
// `>>` or `> >`). Each result carries the whole line (for the gutter border) and
// the content after the markers (muted by the highlight pass). The `>` markers
// stay visible this slice — there is no conceal. Lines inside fenced code blocks
// are skipped (the #zed-09 fence exclusion): a `> ...` line in a code example is
// quoted evidence and stays raw. Lazy continuation lines (quote text without a
// `>`) are deliberately not recognized — only explicit markers qualify.
pub(crate) fn scan_block_quotes(text: &str) -> Vec<YmdBlockQuote> {
    let mut block_quotes = Vec::new();
    let fenced_code_ranges = fenced_code_block_ranges(text);

    for_each_line(text, |line_content, line_start| {
        if range_overlaps_any(
            &(line_start..line_start + line_content.len()),
            &fenced_code_ranges,
        ) {
            return;
        }
        if let Some(block_quote) = scan_line_block_quote(line_content, line_start) {
            block_quotes.push(block_quote);
        }
    });

    block_quotes
}

// Markdown task-list checkboxes (`- [ ] ` unchecked, `- [x] ` checked) after
// optional leading spaces/tabs. Each result's range covers only the marker bytes
// (the editor replaces them with a configurable display character via a visible
// fold); the indentation before it and the task text after it stay raw. Lines
// inside fenced code blocks are skipped (the #zed-09 fence exclusion): a `- [ ]`
// in a code sample is quoted evidence and stays literal. Dialect rules (walk Q5):
// only the dash bullet and a LOWERCASE `x` are recognized — `* [ ]`, `+ [ ]`, and
// `- [X]` stay raw, matching what the surrounding tooling writes.
pub(crate) fn scan_checkboxes(text: &str) -> Vec<YmdCheckbox> {
    let mut checkboxes = Vec::new();
    let fenced_code_ranges = fenced_code_block_ranges(text);

    for_each_line(text, |line_content, line_start| {
        if range_overlaps_any(
            &(line_start..line_start + line_content.len()),
            &fenced_code_ranges,
        ) {
            return;
        }
        if let Some(checkbox) = scan_line_checkbox(line_content, line_start) {
            checkboxes.push(checkbox);
        }
    });

    checkboxes
}

pub(crate) fn scan_conceals(text: &str) -> Vec<YmdConceal> {
    let mut conceals = Vec::new();
    let fenced_code_ranges = fenced_code_block_ranges(text);

    for_each_line(text, |line_content, line_start| {
        if range_overlaps_any(
            &(line_start..line_start + line_content.len()),
            &fenced_code_ranges,
        ) {
            return;
        }
        let inline_highlights = scan_line_background_markups(line_content, line_start);
        for inline_highlight in &inline_highlights {
            conceals.push(YmdConceal {
                range: inline_highlight.open_marker_range.clone(),
            });
            conceals.push(YmdConceal {
                range: inline_highlight.close_marker_range.clone(),
            });
        }

        for link in scan_line_links(line_content, line_start) {
            conceals.push(YmdConceal {
                range: link.open_marker_range,
            });
            conceals.push(YmdConceal {
                range: link.close_marker_range,
            });
        }

        for image in scan_line_images(line_content, line_start) {
            conceals.push(YmdConceal {
                range: image.open_marker_range,
            });
            conceals.push(YmdConceal {
                range: image.close_marker_range,
            });
        }

        for range in scan_line_inline_style_markers(line_content, line_start) {
            conceals.push(YmdConceal { range });
        }

        let heading_range = heading_conceal_range(line_content, line_start);
        if let Some(heading_range) = heading_range.clone() {
            conceals.push(YmdConceal {
                range: heading_range,
            });
        }

        let captures = capture_ranges(&inline_highlights, line_start);
        if let Some((emoji_range, _)) = first_effective_emoji(line_content, &captures) {
            let absolute_emoji_range = line_start + emoji_range.start..line_start + emoji_range.end;
            // When the heading fold has already absorbed the color marker (it sits
            // right after the `#` prefix), the line-marker emoji must not conceal a
            // second time over the same bytes.
            let already_concealed_by_heading = heading_range.is_some_and(|heading_range| {
                heading_range.start <= absolute_emoji_range.start
                    && absolute_emoji_range.end <= heading_range.end
            });
            if !already_concealed_by_heading {
                conceals.push(YmdConceal {
                    range: absolute_emoji_range,
                });
            }
        }
    });

    conceals.sort_by_key(|conceal| (conceal.range.start, conceal.range.end));
    conceals
}

fn capture_ranges(
    inline_highlights: &[YmdInlineHighlight],
    line_start: usize,
) -> Vec<Range<usize>> {
    inline_highlights
        .iter()
        .map(|inline_highlight| {
            inline_highlight.open_marker_range.start - line_start
                ..inline_highlight.close_marker_range.end - line_start
        })
        .collect()
}

// The single traversal both the line-foreground color and the line-marker conceal
// derive from, so the two can never drift: an emoji captured by a valid `==...==`
// pair belongs to the background mechanism only; the FIRST standalone supported
// emoji is the line's marker — it colors the whole line and conceals — and every
// later emoji is content that stays visible.
fn first_effective_emoji(
    line: &str,
    capture_ranges: &[Range<usize>],
) -> Option<(Range<usize>, YmdColor)> {
    let mut search_start = 0;

    while search_start < line.len() {
        let (relative_emoji_start, color, emoji_len) =
            YmdColor::first_match_in(&line[search_start..])?;
        let emoji_start = search_start + relative_emoji_start;

        if capture_ranges
            .iter()
            .any(|range| range.contains(&emoji_start))
        {
            search_start = emoji_start + emoji_len;
        } else {
            return Some((emoji_start..emoji_start + emoji_len, color));
        }
    }

    None
}

// A column-zero ATX heading (`#`×1-6 + whitespace + visible content) conceals its
// opening marker only when the visible content carries a YMD color emoji — plain
// Markdown headings keep their `#`. When the color marker immediately follows the
// prefix, the emoji and its trailing whitespace are absorbed into the fold so the
// revealed heading does not begin with a stray leading space; a marker further into
// the text stays put and is concealed by the line-color mechanism instead. The `⋯`
// in suite headings like `## 🟠⋯ Heading` is plain content, not YMD syntax, so it
// stays visible — the fold ends right after the emoji (here `⋯` directly follows it,
// so there is no whitespace to absorb).
fn heading_conceal_range(line: &str, line_start: usize) -> Option<Range<usize>> {
    let bytes = line.as_bytes();
    let mut hash_count = 0;
    while bytes.get(hash_count) == Some(&b'#') {
        hash_count += 1;
    }

    if hash_count == 0 || hash_count > 6 {
        return None;
    }

    if !matches!(bytes.get(hash_count), Some(b' ' | b'\t')) {
        return None;
    }

    let mut prefix_end = hash_count;
    while matches!(bytes.get(prefix_end), Some(b' ' | b'\t')) {
        prefix_end += 1;
    }

    let heading_text = &line[prefix_end..];
    let color_match = YmdColor::first_match_in(heading_text);
    if heading_text.trim().is_empty() || color_match.is_none() {
        return None;
    }

    let mut conceal_end = prefix_end;
    if let Some((0, _, emoji_len)) = color_match {
        conceal_end += emoji_len;
        while matches!(bytes.get(conceal_end), Some(b' ' | b'\t')) {
            conceal_end += 1;
        }
    }

    Some(line_start..line_start + conceal_end)
}

// Single-line inline Markdown links `[label](url)` with a non-empty label and a
// non-empty URL. Images `![alt](src)` are skipped (the `!` prefix). The URL is
// scanned with CommonMark link-destination paren semantics: parentheses are
// depth-counted, so a balanced `(...)` inside the URL — e.g.
// `…/Rust_(film)` — is part of the destination and the link closes only at the
// matching top-level `)`. Reference-style links and autolinks are out of scope.
fn scan_line_links(line: &str, line_start: usize) -> Vec<YmdLink> {
    let mut links = Vec::new();
    let mut search_start = 0;
    let bytes = line.as_bytes();

    while let Some(open_relative) = line[search_start..].find('[') {
        let open = search_start + open_relative;
        if open > 0 && bytes.get(open - 1) == Some(&b'!') {
            search_start = open + 1;
            continue;
        }

        let label_start = open + 1;
        let Some(close_relative) = line[label_start..].find(']') else {
            break;
        };
        let close = label_start + close_relative;
        if close == label_start || bytes.get(close + 1) != Some(&b'(') {
            search_start = open + 1;
            continue;
        }

        let url_start = close + 2;
        let Some(url_end_relative) = link_destination_end(&line[url_start..]) else {
            search_start = open + 1;
            continue;
        };
        let url_end = url_start + url_end_relative;
        if url_end == url_start {
            search_start = open + 1;
            continue;
        }

        links.push(YmdLink {
            open_marker_range: line_start + open..line_start + label_start,
            label_range: line_start + label_start..line_start + close,
            close_marker_range: line_start + close..line_start + url_end + 1,
        });
        search_start = url_end + 1;
    }

    links
}

fn scan_line_images(line: &str, line_start: usize) -> Vec<YmdImage> {
    let mut images = Vec::new();
    let mut search_start = 0;
    let bytes = line.as_bytes();

    while let Some(open_relative) = line[search_start..].find("![") {
        let open = search_start + open_relative;
        let alt_text_start = open + 2;
        let Some(close_relative) = line[alt_text_start..].find(']') else {
            break;
        };
        let close = alt_text_start + close_relative;
        if close == alt_text_start || bytes.get(close + 1) != Some(&b'(') {
            search_start = open + 2;
            continue;
        }

        let path_start = close + 2;
        let Some(path_end_relative) = link_destination_end(&line[path_start..]) else {
            search_start = open + 2;
            continue;
        };
        let path_end = path_start + path_end_relative;
        if path_end == path_start {
            search_start = open + 2;
            continue;
        }

        images.push(YmdImage {
            open_marker_range: line_start + open..line_start + alt_text_start,
            alt_text_range: line_start + alt_text_start..line_start + close,
            close_marker_range: line_start + close..line_start + path_end + 1,
            path_range: line_start + path_start..line_start + path_end,
            reference_range: line_start + open..line_start + path_end + 1,
        });
        search_start = path_end + 1;
    }

    images
}

fn scan_line_inline_style_markers(line: &str, line_start: usize) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut search_start = 0;

    while search_start < line.len() {
        let Some((open, marker)) = next_inline_style_marker(line, search_start) else {
            break;
        };
        let marker_len = marker.len();
        let content_start = open + marker_len;

        if !is_inline_style_open_boundary(line, open) {
            search_start = content_start;
            continue;
        }
        if is_escaped_at(line, open) || is_inside_inline_code_at(line, open) {
            search_start = content_start;
            continue;
        }

        let mut close_search_start = content_start;
        let mut close = None;
        while let Some(close_relative) = line[close_search_start..].find(marker) {
            let candidate = close_search_start + close_relative;
            let content = &line[content_start..candidate];
            if !is_escaped_at(line, candidate)
                && !is_inside_inline_code_at(line, candidate)
                && is_valid_inline_style_content(content)
                && is_inline_style_close_boundary(line, candidate + marker_len)
            {
                close = Some(candidate);
                break;
            }
            close_search_start = candidate + marker_len;
        }

        let Some(close) = close else {
            search_start = content_start;
            continue;
        };

        ranges.push(line_start + open..line_start + content_start);
        ranges.push(line_start + close..line_start + close + marker_len);
        search_start = close + marker_len;
    }

    ranges
}

fn next_inline_style_marker(line: &str, search_start: usize) -> Option<(usize, &'static str)> {
    let mut next = None;
    for marker in ["**", "~~", "*", "_"] {
        let Some(relative_index) = line[search_start..].find(marker) else {
            continue;
        };
        let index = search_start + relative_index;
        if next.is_none_or(|(next_index, _)| index < next_index) {
            next = Some((index, marker));
        }
    }
    next
}

fn is_inline_style_open_boundary(line: &str, open: usize) -> bool {
    line[..open]
        .chars()
        .next_back()
        .is_none_or(|character| character.is_whitespace() || character.is_ascii_punctuation())
}

fn is_inline_style_close_boundary(line: &str, close_end: usize) -> bool {
    line[close_end..]
        .chars()
        .next()
        .is_none_or(|character| character.is_whitespace() || character.is_ascii_punctuation())
}

fn is_valid_inline_style_content(content: &str) -> bool {
    !content.is_empty()
        && content
            .chars()
            .next()
            .is_some_and(|character| !character.is_whitespace())
        && content
            .chars()
            .next_back()
            .is_some_and(|character| !character.is_whitespace())
}

pub(crate) fn is_escaped_at(line: &str, index: usize) -> bool {
    let bytes = line.as_bytes();
    let mut slash_count = 0;
    let mut cursor = index;
    while cursor > 0 && bytes.get(cursor - 1) == Some(&b'\\') {
        slash_count += 1;
        cursor -= 1;
    }
    slash_count % 2 == 1
}

pub(crate) fn is_inside_inline_code_at(line: &str, index: usize) -> bool {
    let bytes = line.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'`' && !is_escaped_at(line, cursor) {
            let run_len = backtick_run_len(bytes, cursor);
            let open_end = cursor + run_len;
            let mut scan = open_end;
            let mut close = None;

            while scan < bytes.len() {
                if bytes[scan] == b'`' && !is_escaped_at(line, scan) {
                    let close_len = backtick_run_len(bytes, scan);
                    if close_len == run_len {
                        close = Some((scan, close_len));
                        break;
                    }
                    scan += close_len;
                } else {
                    scan += 1;
                }
            }

            if let Some((close_start, close_len)) = close {
                if open_end <= index && index < close_start {
                    return true;
                }
                cursor = close_start + close_len;
            } else {
                cursor = open_end;
            }
        } else {
            cursor += 1;
        }
    }
    false
}

fn backtick_run_len(bytes: &[u8], start: usize) -> usize {
    let mut len = 0;
    while start + len < bytes.len() && bytes[start + len] == b'`' {
        len += 1;
    }
    len
}

// Byte offset of the closing `)` within `after_open` (the slice that begins right
// after `](`), or `None` when no balancing top-level `)` exists on the line. A
// nested `(` raises the depth and its matching `)` lowers it; the destination
// ends at the first `)` seen while depth is zero.
fn link_destination_end(after_open: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, byte) in after_open.bytes().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                if depth == 0 {
                    return Some(offset);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

// One block-quote line: up to three leading spaces, then a run of `>` markers
// (each optionally followed by a single space, so `> > x` nests like `>>x`). The
// content range begins after the last consumed marker/space and runs to the end
// of the line — empty when the line is only markers. Returns `None` when no `>`
// marker is present (the leading-space allowance never matches a plain line). All
// bytes inspected are ASCII, so indices stay on char boundaries.
fn scan_line_block_quote(line: &str, line_start: usize) -> Option<YmdBlockQuote> {
    let bytes = line.as_bytes();
    let mut index = fence_start_index(bytes)?;

    let mut depth = 0;
    while bytes.get(index) == Some(&b'>') {
        depth += 1;
        index += 1;
        if bytes.get(index) == Some(&b' ') {
            index += 1;
        }
    }

    (depth > 0).then(|| YmdBlockQuote {
        line_range: line_start..line_start + line.len(),
        content_range: line_start + index..line_start + line.len(),
    })
}

// One task-list line: optional leading spaces/tabs, then exactly `- [ ] ` or
// `- [x] ` (the trailing space is required, so `- [ ]end` with no gap is not a
// task). The returned range spans only the six marker bytes. All bytes inspected
// are ASCII, so the range edges land on char boundaries (the conceal crash family
// is byte/char-boundary, so this is load-bearing). The slice is taken with
// `get(..)` rather than indexing so a short line cannot panic.
fn scan_line_checkbox(line: &str, line_start: usize) -> Option<YmdCheckbox> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while matches!(bytes.get(index), Some(b' ' | b'\t')) {
        index += 1;
    }

    let checked = match bytes.get(index..index + 6)? {
        b"- [ ] " => false,
        b"- [x] " => true,
        _ => return None,
    };

    Some(YmdCheckbox {
        range: line_start + index..line_start + index + 6,
        checked,
    })
}

fn scan_line_background_markups(line: &str, line_start: usize) -> Vec<YmdInlineHighlight> {
    let mut inline_highlights = Vec::new();
    let mut search_start = 0;

    while let Some(open_relative) = line[search_start..].find("==") {
        let open = search_start + open_relative;
        let content_start = open + 2;
        let Some(close_relative) = line[content_start..].find("==") else {
            break;
        };
        let close = content_start + close_relative;
        let content = &line[content_start..close];

        if !content.is_empty() {
            let (color, marker_len) =
                YmdColor::from_emoji(content).unwrap_or((YmdColor::Default, 0));
            let highlight_start = content_start + marker_len;
            if highlight_start < close {
                inline_highlights.push(YmdInlineHighlight {
                    range: line_start + highlight_start..line_start + close,
                    open_marker_range: line_start + open..line_start + content_start + marker_len,
                    close_marker_range: line_start + close..line_start + close + 2,
                    color,
                });
            }
        }

        search_start = close + 2;
    }

    inline_highlights
}

#[derive(Clone, Copy)]
struct CodeFence {
    marker: u8,
    length: usize,
}

// Byte ranges of fenced code blocks (``` or ~~~), delimiter lines included, so YMD
// styling never applies inside them — code is quoted evidence and its punctuation,
// emoji, markers, and URLs must stay literal (#zed-09, value-mode CORRECTNESS). A
// scanner-local CommonMark heuristic, deliberately simpler than tree-sitter: up to
// three leading spaces, then a run of >=3 backticks or tildes; a backtick opener
// may not carry a backtick in its info string; the block closes on a same-marker
// run at least as long, followed only by whitespace, or runs to end-of-buffer while
// unclosed (so in-progress code stays literal as it is typed). Indented code blocks
// and inline code spans are out of scope (the live Future item is fences; inline
// spans are `#zed-310`).
fn fenced_code_block_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut opening_fence: Option<(usize, CodeFence)> = None;
    let mut line_start = 0;

    for line in text.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let line_content_end = line_end - usize::from(line.ends_with('\n'));
        let line_content = &text[line_start..line_content_end];

        if let Some((block_start, fence)) = opening_fence {
            if is_closing_code_fence(line_content, fence) {
                ranges.push(block_start..line_end);
                opening_fence = None;
            }
        } else if let Some(fence) = opening_code_fence(line_content) {
            opening_fence = Some((line_start, fence));
        }

        line_start = line_end;
    }

    // An unclosed fence excludes the rest of the buffer.
    if let Some((block_start, _)) = opening_fence {
        ranges.push(block_start..text.len());
    }

    ranges
}

fn range_overlaps_any(range: &Range<usize>, ranges: &[Range<usize>]) -> bool {
    ranges
        .iter()
        .any(|excluded| range.start < excluded.end && range.end > excluded.start)
}

fn opening_code_fence(line: &str) -> Option<CodeFence> {
    let bytes = line.as_bytes();
    let fence_start = fence_start_index(bytes)?;
    let marker = *bytes.get(fence_start)?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }

    let length = bytes[fence_start..]
        .iter()
        .take_while(|byte| **byte == marker)
        .count();
    if length < 3 {
        return None;
    }

    // CommonMark: a backtick opener's info string may not contain a backtick, so a
    // line like ```text with `inline` is not a fence opener.
    let info_string = &bytes[fence_start + length..];
    if marker == b'`' && info_string.contains(&b'`') {
        return None;
    }

    Some(CodeFence { marker, length })
}

fn is_closing_code_fence(line: &str, opening_fence: CodeFence) -> bool {
    let bytes = line.as_bytes();
    let Some(fence_start) = fence_start_index(bytes) else {
        return false;
    };
    if bytes.get(fence_start) != Some(&opening_fence.marker) {
        return false;
    }

    let length = bytes[fence_start..]
        .iter()
        .take_while(|byte| **byte == opening_fence.marker)
        .count();
    if length < opening_fence.length {
        return false;
    }

    bytes[fence_start + length..]
        .iter()
        .all(|byte| matches!(*byte, b' ' | b'\t'))
}

fn fence_start_index(bytes: &[u8]) -> Option<usize> {
    let mut index = 0;
    while bytes.get(index) == Some(&b' ') {
        index += 1;
        if index > 3 {
            return None;
        }
    }
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_default_background_highlight() {
        assert_eq!(
            scan("==hello=="),
            vec![YmdHighlight {
                range: 2..7,
                kind: YmdHighlightKind::Background(YmdColor::Default),
            }]
        );
    }

    #[test]
    fn scans_default_conceal_ranges() {
        assert_eq!(
            scan_conceals("==hello=="),
            vec![YmdConceal { range: 0..2 }, YmdConceal { range: 7..9 },]
        );
    }

    #[test]
    fn scans_emoji_background_highlight_without_emoji_marker() {
        assert_eq!(
            scan("==🔴urgent=="),
            vec![YmdHighlight {
                range: 6..12,
                kind: YmdHighlightKind::Background(YmdColor::Red),
            }]
        );
    }

    #[test]
    fn scans_emoji_conceal_ranges_with_emoji_marker() {
        assert_eq!(
            scan_conceals("==🔴urgent=="),
            vec![YmdConceal { range: 0..6 }, YmdConceal { range: 12..14 },]
        );
    }

    #[test]
    fn invalid_markers_stay_raw_but_their_emoji_still_conceals() {
        assert!(scan("====").is_empty());
        assert!(scan("==missing close").is_empty());
        assert!(scan_conceals("====").is_empty());
        assert!(scan_conceals("==missing close").is_empty());
        // The invalid markers stay raw, but their emoji is standalone (the line's
        // marker) and conceals like any other line-color emoji.
        assert_eq!(scan_conceals("==🔴=="), vec![YmdConceal { range: 2..6 }]);
        assert_eq!(scan_conceals("==🔴 open"), vec![YmdConceal { range: 2..6 }]);
    }

    #[test]
    fn emoji_in_invalid_markers_colors_the_line() {
        assert_eq!(
            scan("==🔴 open"),
            vec![YmdHighlight {
                range: 0..11,
                kind: YmdHighlightKind::LineForeground(YmdColor::Red),
            }]
        );
        assert_eq!(
            scan("==🔴=="),
            vec![YmdHighlight {
                range: 0..8,
                kind: YmdHighlightKind::LineForeground(YmdColor::Red),
            }]
        );
    }

    #[test]
    fn standalone_and_marker_emoji_compose_independently() {
        assert_eq!(
            scan("🔴 ==🟡text=="),
            vec![
                YmdHighlight {
                    range: 11..15,
                    kind: YmdHighlightKind::Background(YmdColor::Yellow),
                },
                YmdHighlight {
                    range: 0..17,
                    kind: YmdHighlightKind::LineForeground(YmdColor::Red),
                },
            ]
        );
    }

    #[test]
    fn mid_content_emoji_is_span_content() {
        // Tom's ruling: only the marker-position emoji (right after `==`) picks the
        // highlight color; an emoji elsewhere inside a valid pair is plain span text
        // and influences neither the background color nor the line foreground.
        assert_eq!(
            scan("==a🔴b=="),
            vec![YmdHighlight {
                range: 2..8,
                kind: YmdHighlightKind::Background(YmdColor::Default),
            }]
        );
        assert_eq!(
            scan_conceals("==a🔴b=="),
            vec![YmdConceal { range: 0..2 }, YmdConceal { range: 8..10 },]
        );
    }

    #[test]
    fn removed_and_near_miss_emoji_stay_inert() {
        assert!(scan("⚪ white removed").is_empty());
        assert!(scan("🟤 brown removed").is_empty());
        assert!(scan("🟦 square never supported").is_empty());
        assert_eq!(
            scan("==⚪text==\n==🟤text=="),
            vec![
                YmdHighlight {
                    range: 2..9,
                    kind: YmdHighlightKind::Background(YmdColor::Default),
                },
                YmdHighlight {
                    range: 14..22,
                    kind: YmdHighlightKind::Background(YmdColor::Default),
                },
            ]
        );
    }

    #[test]
    fn scans_orange_purple_and_black() {
        assert_eq!(
            scan("==🟠note=="),
            vec![YmdHighlight {
                range: 6..10,
                kind: YmdHighlightKind::Background(YmdColor::Orange),
            }]
        );
        assert_eq!(
            scan("🟣 plan"),
            vec![YmdHighlight {
                range: 0..9,
                kind: YmdHighlightKind::LineForeground(YmdColor::Purple),
            }]
        );
        assert_eq!(
            scan("⚫ done"),
            vec![YmdHighlight {
                range: 0..8,
                kind: YmdHighlightKind::LineForeground(YmdColor::Black),
            }]
        );
    }

    #[test]
    fn stops_background_highlight_at_first_close_marker() {
        assert_eq!(
            scan("==a == b=="),
            vec![YmdHighlight {
                range: 2..4,
                kind: YmdHighlightKind::Background(YmdColor::Default),
            }]
        );
    }

    #[test]
    fn does_not_scan_background_highlights_across_lines() {
        // The opening row's emoji sits in an invalid (unterminated) marker, so it
        // line-colors that row; no background span may cross the line break.
        assert_eq!(
            scan("==🔵 starts\nends=="),
            vec![YmdHighlight {
                range: 0..13,
                kind: YmdHighlightKind::LineForeground(YmdColor::Blue),
            }]
        );
        assert_eq!(
            scan_conceals("==🔵 starts\nends=="),
            vec![YmdConceal { range: 2..6 }]
        );
    }

    #[test]
    fn conceal_output_is_sorted_by_range() {
        // The line-marker emoji precedes the valid pair positionally but is
        // pushed after the pair's marker conceals; the sort restores range
        // order, which the editor's diff sync and fold insertion rely on.
        assert_eq!(
            scan_conceals("🔴 ==🟡text=="),
            vec![
                YmdConceal { range: 0..4 },
                YmdConceal { range: 5..11 },
                YmdConceal { range: 15..17 },
            ]
        );
    }

    #[test]
    fn scans_line_foreground_emoji_conceal_ranges() {
        assert_eq!(
            scan_conceals("🔴Emoji hidden\nplain 🔵 line"),
            vec![YmdConceal { range: 0..4 }, YmdConceal { range: 23..27 }]
        );
    }

    #[test]
    fn does_not_double_conceal_inline_highlight_emoji() {
        // The captured 🔴 conceals once with its opening marker; the standalone
        // 🔵 is the line's marker and conceals separately.
        assert_eq!(
            scan_conceals("==🔴urgent== and 🔵 line"),
            vec![
                YmdConceal { range: 0..6 },
                YmdConceal { range: 12..14 },
                YmdConceal { range: 19..23 },
            ]
        );
    }

    #[test]
    fn one_marker_per_line_conceals_only_the_first_emoji() {
        // The first effective standalone emoji is the marker (colors + conceals);
        // every later emoji is content and stays visible.
        assert_eq!(
            scan_conceals("🔴 red wins before 🟢 green"),
            vec![YmdConceal { range: 0..4 }]
        );
        assert_eq!(
            scan("🔴 red wins before 🟢 green"),
            vec![YmdHighlight {
                range: 0..31,
                kind: YmdHighlightKind::LineForeground(YmdColor::Red),
            }]
        );
    }

    #[test]
    fn invalid_marker_emoji_is_the_line_marker_in_composites() {
        // `==🔴== 🟢 text`: 🔴 sits in an INVALID (empty-span) marker, so it is
        // standalone and FIRST — it drives the line color and conceals; 🟢 stays
        // visible content; the invalid `====` stays raw.
        assert_eq!(
            scan("==🔴== 🟢 text"),
            vec![YmdHighlight {
                range: 0..18,
                kind: YmdHighlightKind::LineForeground(YmdColor::Red),
            }]
        );
        assert_eq!(
            scan_conceals("==🔴== 🟢 text"),
            vec![YmdConceal { range: 2..6 }]
        );
    }

    #[test]
    fn conceals_emoji_heading_prefix_with_absorbed_marker() {
        // The color marker sits right after the `#` prefix, so the fold absorbs
        // `#`, whitespace, the emoji, and the emoji's trailing whitespace — the
        // revealed heading begins with its first letter, not a stray space. A tab
        // separator counts as heading whitespace too.
        assert_eq!(
            scan_conceals("# 🔵 Blue\n##\t🟢 Green"),
            vec![YmdConceal { range: 0..7 }, YmdConceal { range: 12..20 }]
        );
    }

    #[test]
    fn conceals_each_heading_level_clean_and_dots_variant() {
        // Every level H1..H6 hides only its opening marker. The clean variant
        // absorbs the immediate emoji; the `⋯` variant stops the fold at the
        // emoji's trailing whitespace so the `⋯` — plain content, not YMD syntax —
        // stays visible. The blue emoji is 4 bytes; `#`×n + one space + 4 + one
        // space = n + 6, and the `⋯` variant has no trailing space so it is n + 5.
        for level in 1..=6usize {
            let hashes = "#".repeat(level);
            let clean = format!("{hashes} 🔵 Heading");
            assert_eq!(
                scan_conceals(&clean),
                vec![YmdConceal {
                    range: 0..level + 6
                }],
                "clean H{level}"
            );

            let dots = format!("{hashes} 🔵⋯ Heading");
            assert_eq!(
                scan_conceals(&dots),
                vec![YmdConceal {
                    range: 0..level + 5
                }],
                "dots H{level}"
            );
        }
    }

    #[test]
    fn dots_after_heading_marker_stay_visible_not_concealed() {
        // PIN (guards `#zed-09`'s fence rework): in `## 🟠⋯ Heading` the fold ends
        // right after `🟠` (byte 7) — `⋯` directly follows the emoji, no whitespace
        // to absorb. The `⋯` (bytes 7..10) is NOT in any conceal range — it is
        // content, never YMD syntax. If a future change ever folds the `⋯`, this
        // assertion fails first.
        let conceals = scan_conceals("## 🟠⋯ Heading");
        assert_eq!(conceals, vec![YmdConceal { range: 0..7 }]);
        let dots_range = 7..("## 🟠⋯".len());
        assert!(
            !conceals
                .iter()
                .any(|conceal| conceal.range.start < dots_range.end
                    && dots_range.start < conceal.range.end),
            "the ⋯ must stay visible — it is content, not YMD syntax"
        );
    }

    #[test]
    fn emoji_only_heading_is_fully_absorbed() {
        // `# 🔵` with no visible text after the emoji: the whole line is the marker,
        // so concealment absorbs all of it and the heading vanishes off-cursor. The
        // outline panel still shows the raw text (it derives from buffer text).
        assert_eq!(scan_conceals("# 🔵"), vec![YmdConceal { range: 0..6 }]);
    }

    #[test]
    fn heading_with_mid_text_emoji_hides_prefix_and_marker_separately() {
        // Composite (a): `# Blue 🔵 Heading`. The emoji is NOT adjacent to the
        // prefix, so the heading fold takes only `# ` (0..2). The mid-text 🔵 is the
        // line's color marker and conceals on its own (`#zed-04`), leaving the
        // documented double space in clean view (residue is `#zed-308`'s concern).
        assert_eq!(
            scan_conceals("# Blue 🔵 Heading"),
            vec![YmdConceal { range: 0..2 }, YmdConceal { range: 7..11 }]
        );
    }

    #[test]
    fn does_not_conceal_plain_or_invalid_heading_prefixes() {
        // Heading-gate boundary cases (the emoji-only composite is pinned by
        // `emoji_only_heading_is_fully_absorbed`): a plain heading keeps its `#`;
        // `#heading` with no separating space is not a heading (the trailing 🔵 still
        // line-color conceals); seven `#` is too deep; an empty heading has no
        // content.
        assert!(scan_conceals("# Plain Heading").is_empty());
        assert_eq!(
            scan_conceals("#heading 🔵"),
            vec![YmdConceal { range: 9..13 }]
        );
        assert_eq!(
            scan_conceals("####### 🔵 Too Deep"),
            vec![YmdConceal { range: 8..12 }]
        );
        assert!(scan_conceals("# \t").is_empty());
        // A heading whose content has no color emoji keeps its `#`.
        assert!(scan_conceals("# No Color Heading").is_empty());
    }

    #[test]
    fn emoji_inside_valid_marker_still_gates_heading_prefix() {
        // PIN (C1, guards `#zed-09`'s fence rework): the heading gate runs on the raw
        // `first_match_in`, which — unlike `first_effective_emoji` — does NOT exclude
        // an emoji captured inside a valid `==🔴…==` background marker. So a heading
        // whose only color emoji lives inside such a marker STILL folds its `# `
        // prefix (intended under the emoji-anywhere ruling). The emoji is not at the
        // heading text's offset 0 (the `==` precede it), so only `# ` (0..2) folds;
        // the `==🔴urgent==` pair conceals its own open marker (`==🔴`, 2..8) and
        // close marker (14..16) — adjacent to the heading fold, never overlapping it,
        // and the in-marker 🔴 never double-conceals.
        assert_eq!(
            scan_conceals("# ==🔴urgent== rest"),
            vec![
                YmdConceal { range: 0..2 },
                YmdConceal { range: 2..8 },
                YmdConceal { range: 14..16 },
            ]
        );
    }

    #[test]
    fn first_standalone_line_foreground_emoji_wins() {
        assert_eq!(
            scan("hello 🔵 world 🔴\nplain"),
            vec![YmdHighlight {
                range: 0..21,
                kind: YmdHighlightKind::LineForeground(YmdColor::Blue),
            }]
        );
        assert_eq!(
            scan("==🔴x== hello 🔵 world 🟢"),
            vec![
                YmdHighlight {
                    range: 6..7,
                    kind: YmdHighlightKind::Background(YmdColor::Red),
                },
                YmdHighlight {
                    range: 0..31,
                    kind: YmdHighlightKind::LineForeground(YmdColor::Blue),
                },
            ]
        );
    }

    #[test]
    fn scans_basic_markdown_link() {
        assert_eq!(
            scan_links("[link](url)"),
            vec![YmdLink {
                open_marker_range: 0..1,
                label_range: 1..5,
                close_marker_range: 5..11,
            }]
        );
        // Only the brackets and the `](url)` tail conceal; the label stays visible.
        assert_eq!(
            scan_conceals("[link](url)"),
            vec![YmdConceal { range: 0..1 }, YmdConceal { range: 5..11 }]
        );
    }

    #[test]
    fn scans_inline_style_marker_conceal_ranges() {
        assert_eq!(
            scan_conceals("**closed** *paused* _open_ ~~cancelled~~"),
            vec![
                YmdConceal { range: 0..2 },
                YmdConceal { range: 8..10 },
                YmdConceal { range: 11..12 },
                YmdConceal { range: 18..19 },
                YmdConceal { range: 20..21 },
                YmdConceal { range: 25..26 },
                YmdConceal { range: 27..29 },
                YmdConceal { range: 38..40 },
            ]
        );
        assert!(scan_conceals("snake_case_more").is_empty());
        assert!(scan_conceals("* missing close").is_empty());
        assert!(scan_conceals("* untrimmed *").is_empty());
        assert!(scan_conceals(r"\*literal*").is_empty());
        assert!(scan_conceals(r"*literal\*").is_empty());
        assert!(scan_conceals("`*literal*`").is_empty());
        assert!(scan_conceals("``*literal*``").is_empty());
        assert!(scan_conceals("```_literal_```").is_empty());
    }

    #[test]
    fn scans_multiple_markdown_links_per_line_and_skips_images() {
        // The image `![alt](src)` is skipped; the two real links conceal
        // independently, each leaving its own label visible.
        assert_eq!(
            scan_links("![alt](src) [ok](url) and [two](2)"),
            vec![
                YmdLink {
                    open_marker_range: 12..13,
                    label_range: 13..15,
                    close_marker_range: 15..21,
                },
                YmdLink {
                    open_marker_range: 26..27,
                    label_range: 27..30,
                    close_marker_range: 30..34,
                },
            ]
        );
    }

    #[test]
    fn scans_markdown_images() {
        assert_eq!(
            scan_images("![alt](src) text ![two](assets/two.png)"),
            vec![
                YmdImage {
                    open_marker_range: 0..2,
                    alt_text_range: 2..5,
                    close_marker_range: 5..11,
                    path_range: 7..10,
                    reference_range: 0..11,
                },
                YmdImage {
                    open_marker_range: 17..19,
                    alt_text_range: 19..22,
                    close_marker_range: 22..39,
                    path_range: 24..38,
                    reference_range: 17..39,
                },
            ]
        );
        assert_eq!(
            scan_conceals("![alt](src)"),
            vec![YmdConceal { range: 0..2 }, YmdConceal { range: 5..11 }]
        );
    }

    #[test]
    fn scans_balanced_paren_image_path_as_single_image() {
        let line = "![Rust logo](assets/Rust_(logo).png)";
        let close = line.find("](").unwrap();
        let images = scan_images(line);
        assert_eq!(
            images,
            vec![YmdImage {
                open_marker_range: 0..2,
                alt_text_range: 2..close,
                close_marker_range: close..line.len(),
                path_range: close + 2..line.len() - 1,
                reference_range: 0..line.len(),
            }]
        );
        assert_eq!(images[0].close_marker_range.end, line.len());
    }

    #[test]
    fn ignores_invalid_markdown_images() {
        assert!(scan_images("![](url)").is_empty());
        assert!(scan_images("![missing url]()").is_empty());
        assert!(scan_images("![no closing paren](url").is_empty());
        assert!(scan_images("![brackets only]").is_empty());
    }

    #[test]
    fn scans_balanced_paren_url_as_single_link() {
        // CommonMark link-destination parens are depth-counted: the balanced
        // `(film)` inside the URL belongs to the destination, so the whole
        // `](…/Rust_(film))` tail conceals and NO stray `)` is left visible.
        let line = "[Rust (film)](https://en.wikipedia.org/wiki/Rust_(film))";
        let close = line.find("](").unwrap();
        let links = scan_links(line);
        assert_eq!(
            links,
            vec![YmdLink {
                open_marker_range: 0..1,
                label_range: 1..close,
                close_marker_range: close..line.len(),
            }]
        );
        // The close marker reaches the final byte — the trailing `)` is folded,
        // not orphaned.
        assert_eq!(links[0].close_marker_range.end, line.len());
        assert_eq!(
            scan_conceals(line),
            vec![
                YmdConceal { range: 0..1 },
                YmdConceal {
                    range: close..line.len()
                },
            ]
        );
    }

    #[test]
    fn link_inside_highlight_folds_without_overlapping_highlight_markers() {
        // Suite composite `==🔴 [link inside highlight](url)==`: the link folds
        // live strictly between the `==🔴` open marker and the `==` close marker,
        // so the link conceals and the highlight conceals never overlap (this is
        // the exact no-overlap claim the brief makes — bounded to this case).
        let line = "==🔴 [link inside highlight](url)==";
        let open_bracket = line.find('[').unwrap();
        let close_bracket = line.find(']').unwrap();
        let url_close = line.rfind(')').unwrap();
        assert_eq!(
            scan_links(line),
            vec![YmdLink {
                open_marker_range: open_bracket..open_bracket + 1,
                label_range: open_bracket + 1..close_bracket,
                close_marker_range: close_bracket..url_close + 1,
            }]
        );

        let conceals = scan_conceals(line);
        // Highlight open `==🔴` (0..6) and close `==` conceal; the link `[`
        // (open_bracket..+1) and `](url)` (close_bracket..url_close+1) conceal.
        // None of the link ranges intersect the highlight marker ranges.
        let highlight_open = 0..6;
        let highlight_close = (line.len() - 2)..line.len();
        let link_open = open_bracket..open_bracket + 1;
        let link_close = close_bracket..url_close + 1;
        for conceal in &conceals {
            if conceal.range == link_open || conceal.range == link_close {
                assert!(
                    !(conceal.range.start < highlight_open.end
                        && highlight_open.start < conceal.range.end),
                    "link fold overlaps highlight open marker"
                );
                assert!(
                    !(conceal.range.start < highlight_close.end
                        && highlight_close.start < conceal.range.end),
                    "link fold overlaps highlight close marker"
                );
            }
        }
        assert!(conceals.contains(&YmdConceal { range: link_open }));
        assert!(conceals.contains(&YmdConceal { range: link_close }));
    }

    #[test]
    fn nested_highlight_markers_in_url_do_not_break_link_scan() {
        // A `==` sequence sitting inside the URL is just URL bytes to the link
        // scanner — the link still scans as one unit and its destination is taken
        // verbatim up to the balancing `)`.
        let line = "[label](http://x/==a==)";
        let close = line.find("](").unwrap();
        assert_eq!(
            scan_links(line),
            vec![YmdLink {
                open_marker_range: 0..1,
                label_range: 1..close,
                close_marker_range: close..line.len(),
            }]
        );
    }

    #[test]
    fn link_style_underline_inherits_the_label_color() {
        // The link underline sets NO color override, so it inherits the label
        // text color (the Markdown link blue) and matches the revealed link
        // instead of a pinned color. #zed-35 dogfood reversed #zed-06's Q4
        // pinned-default-foreground choice (blue text under a near-black line).
        let underline = link_style()
            .underline
            .expect("link style sets an underline");
        assert_eq!(
            underline.color, None,
            "underline must inherit the text color, not pin its own"
        );
    }

    #[test]
    fn url_range_excludes_the_close_markers() {
        // `url_range()` is the destination bytes only — the leading `](` and the
        // trailing `)` stripped. Markers are single ASCII bytes, so it stays on
        // char boundaries. Backs #zed-35's URL recovery.
        let line = "[label](https://example.com)";
        let link = scan_links(line).remove(0);
        assert_eq!(&line[link.url_range()], "https://example.com");
    }

    #[test]
    fn markdown_link_url_at_offset_resolves_label_to_url() {
        // A click anywhere inside the visible label resolves to the concealed URL
        // plus the label range (the hover underline target). #zed-35.
        let line = "[Zed RBF](https://zed.dev)";
        let label_start = 1;
        let label_end = line.find("](").unwrap();
        for offset in [label_start, label_start + 2, label_end - 1] {
            assert_eq!(
                markdown_link_url_at_offset(line, offset),
                Some((label_start..label_end, "https://zed.dev".to_string())),
                "offset {offset} inside the label should resolve to the URL"
            );
        }
    }

    #[test]
    fn markdown_link_url_at_offset_takes_whole_balanced_paren_url() {
        // The depth-counted destination is recovered whole — the inner `(film)`
        // is part of the URL, not a truncation point.
        let line = "[Rust (film)](https://en.wikipedia.org/wiki/Rust_(film))";
        let label_end = line.find("](").unwrap();
        let (label_range, url) = markdown_link_url_at_offset(line, 3).unwrap();
        assert_eq!(label_range, 1..label_end);
        assert_eq!(url, "https://en.wikipedia.org/wiki/Rust_(film)");
    }

    #[test]
    fn markdown_link_url_at_offset_picks_the_label_under_the_offset() {
        // Two links on one line: each label resolves only to its own URL.
        let line = "[first](url-1) and [second](url-2)";
        assert_eq!(
            markdown_link_url_at_offset(line, line.find("first").unwrap()).map(|(_, url)| url),
            Some("url-1".to_string())
        );
        assert_eq!(
            markdown_link_url_at_offset(line, line.find("second").unwrap()).map(|(_, url)| url),
            Some("url-2".to_string())
        );
    }

    #[test]
    fn markdown_link_url_at_offset_rejects_non_label_offsets() {
        let line = "[label](https://example.com)";
        // The `[` open bracket (offset 0) is a conceal marker, not the label.
        assert_eq!(markdown_link_url_at_offset(line, 0), None);
        // Inside the URL — the raw URL is `find_url`'s job, not this path.
        assert_eq!(
            markdown_link_url_at_offset(line, line.find("example").unwrap()),
            None
        );
        // Plain text with no link at all.
        assert_eq!(markdown_link_url_at_offset("just prose", 3), None);
    }

    #[test]
    fn markdown_link_url_at_offset_skips_fenced_code_and_images() {
        // A link inside a fenced code block is NOT concealed (#zed-09), so its
        // label is not specially clickable — reusing `scan_links` keeps the
        // clickable set identical to the concealed set. An image label is not a
        // text link.
        let fenced = "```\n[x](https://example.com)\n```";
        assert_eq!(
            markdown_link_url_at_offset(fenced, fenced.find('x').unwrap()),
            None
        );
        let image = "![alt](https://example.com/i.png)";
        assert_eq!(
            markdown_link_url_at_offset(image, image.find("alt").unwrap()),
            None
        );
    }

    #[test]
    fn ignores_invalid_markdown_links() {
        // In assert order: empty URL `[x]()`, missing closing paren `[x](url`, no
        // `](` destination at all `[brackets only]`, and an empty label `[](url)`
        // all stay raw (no conceals, no link).
        assert!(scan_links("[missing url]()").is_empty());
        assert!(scan_links("[no closing paren](url").is_empty());
        assert!(scan_links("[brackets only]").is_empty());
        assert!(scan_links("[](url)").is_empty());
        assert!(scan_conceals("[missing url]()").is_empty());
        assert!(scan_conceals("[no closing paren](url").is_empty());
    }

    #[test]
    fn link_destination_stops_at_first_top_level_paren_and_keeps_trailing_text() {
        // `link_destination_end` ends the URL at the first depth-zero `)` — the
        // trailing prose after the link is NEVER swallowed into the close marker.
        // Pins the over-consume boundary so a future depth-rule tweak can't start
        // eating text past the link.
        let line = "[a](u) trailing";
        assert_eq!(
            scan_links(line),
            vec![YmdLink {
                open_marker_range: 0..1,
                label_range: 1..2,
                // `](u)` only — ends at byte 6, not the end of the line.
                close_marker_range: 2..6,
            }]
        );
        // Only the brackets and `](u)` fold; ` trailing` stays visible.
        assert_eq!(
            scan_conceals(line),
            vec![YmdConceal { range: 0..1 }, YmdConceal { range: 2..6 }]
        );

        // The scan resumes right after the first link's `)`, so a second link
        // later on the line is found independently (no over-consume across them).
        assert_eq!(
            scan_links("[a](u) and [b](v)"),
            vec![
                YmdLink {
                    open_marker_range: 0..1,
                    label_range: 1..2,
                    close_marker_range: 2..6,
                },
                YmdLink {
                    open_marker_range: 11..12,
                    label_range: 12..13,
                    close_marker_range: 13..17,
                },
            ]
        );
    }

    #[test]
    fn ignores_reference_style_links_and_autolinks() {
        // Q6 permanent boundary: reference-style links (`[label][ref]`, plus its
        // `[ref]: url` definition line) and autolinks (`<https://…>`) are a
        // different grammar — not inline `[label](url)` — so they are never
        // concealed. Locks the boundary the brief claims.
        assert!(scan_links("[label][ref]").is_empty());
        assert!(scan_links("[ref]: https://example.com").is_empty());
        assert!(scan_links("<https://x.com>").is_empty());
        assert!(scan_conceals("[label][ref]").is_empty());
        assert!(scan_conceals("<https://x.com>").is_empty());
    }

    #[test]
    fn scans_link_with_query_and_fragment_url() {
        // Query strings and fragments are just destination bytes (no `?`/`#`/`&`
        // special-casing), so the whole `](…)` tail folds. Retires the
        // real-document "query strings, fragments" risk in code.
        let line = "[x](https://e.com/p?q=1&r=2#frag)";
        let close = line.find("](").unwrap();
        let links = scan_links(line);
        assert_eq!(
            links,
            vec![YmdLink {
                open_marker_range: 0..1,
                label_range: 1..close,
                close_marker_range: close..line.len(),
            }]
        );
        assert_eq!(links[0].close_marker_range.end, line.len());
    }

    #[test]
    fn scans_thematic_breaks() {
        // The three accepted forms: a bare `---`, a longer `----`, and the spaced
        // `- - -` (with up to 3 leading spaces). Each range covers the line content
        // only, no trailing newline.
        assert_eq!(
            scan_thematic_breaks("text\n---\nmore\n  - - -\n----"),
            vec![5..8, 14..21, 22..26]
        );
    }

    #[test]
    fn ignores_non_break_dash_lines() {
        // Two dashes, a dash line with trailing text, and a code-indented (4 spaces)
        // run all stay raw.
        assert!(scan_thematic_breaks("--").is_empty());
        assert!(scan_thematic_breaks("--- text").is_empty());
        assert!(scan_thematic_breaks("    ---").is_empty());
    }

    #[test]
    fn star_and_underscore_thematic_breaks_stay_raw() {
        // PIN (walk Q2 — permanent dialect rule): only the dash (`-`) family renders
        // as a rule. CommonMark also treats `***`, `___`, and their spaced/longer
        // variants as thematic breaks, but this editor deliberately leaves them raw.
        // If a future change ever recognizes `*`/`_`, this assertion fails first.
        assert!(scan_thematic_breaks("***").is_empty());
        assert!(scan_thematic_breaks("___").is_empty());
        assert!(scan_thematic_breaks("* * *").is_empty());
        assert!(scan_thematic_breaks("_____").is_empty());
        // A dash break on a later line still renders — only the `*`/`_` lines are inert.
        assert_eq!(scan_thematic_breaks("***\n---"), vec![4..7]);
    }

    #[test]
    fn skips_yaml_frontmatter_break_delimiters() {
        // A break-shaped line 1 opens a frontmatter skip; the opening and closing
        // delimiters are both skipped, and only a body `---` after them renders.
        let text = "---\ntitle: Test\n---\nbody\n---";
        let ranges = scan_thematic_breaks(text);
        assert_eq!(ranges, vec![25..28]);
        assert_eq!(&text[ranges[0].clone()], "---");
    }

    #[test]
    fn rule_immediately_after_frontmatter_renders() {
        // Pins the `>=` boundary in `scan_thematic_breaks`: the closing `---`'s
        // line_end equals the next line's line_start, so a `---` on the very line
        // after the frontmatter close is NOT swallowed by the skip — it renders.
        let text = "---\nk: v\n---\n---\nx";
        let ranges = scan_thematic_breaks(text);
        assert_eq!(ranges, vec![13..16]);
        assert_eq!(&text[ranges[0].clone()], "---");
    }

    #[test]
    fn frontmatter_less_opener_loses_two_rules_loose_bet() {
        // Documents the deliberate loose-opener bet (walk Q3, Tom's override of the
        // tighten rec): there is NO real frontmatter here, but because line 1 is
        // break-shaped it opens the skip, and the next break-shaped line is read as
        // the closing delimiter. So a document that genuinely opens with a rule loses
        // BOTH that rule's replacement and the next one — only the third `---` renders.
        let text = "---\n---\n---";
        assert_eq!(scan_thematic_breaks(text), vec![8..11]);
        // A non-break line 1 means no skip at all — every `---` renders.
        let text = "intro\n---\n---";
        assert_eq!(scan_thematic_breaks(text), vec![6..9, 10..13]);
    }

    #[test]
    fn skips_all_ymd_features_inside_fenced_code_blocks() {
        let text = "==outside==\n```lua\n🔴 code\n==🔵 inside==\n[inside](url)\n![inside](image.png)\n---\n```\n==after==\n[after](url)\n![after](image.png)\n---";
        let fences = fenced_code_block_ranges(text);
        assert_eq!(fences.len(), 1);

        // Highlights: only the two outside `==...==` spans color.
        let backgrounds = scan(text)
            .into_iter()
            .filter_map(|highlight| match highlight.kind {
                YmdHighlightKind::Background(_) => Some(text[highlight.range].to_string()),
                YmdHighlightKind::LineForeground(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(backgrounds, vec!["outside", "after"]);

        // Every scanner entrypoint excludes the fenced lines: highlights, conceals,
        // link underlines, and thematic-break rules all skip the fence.
        assert!(
            scan(text)
                .iter()
                .all(|h| !range_overlaps_any(&h.range, &fences))
        );
        assert!(
            scan_conceals(text)
                .iter()
                .all(|c| !range_overlaps_any(&c.range, &fences))
        );
        assert!(
            scan_links(text)
                .iter()
                .all(|l| !range_overlaps_any(&l.label_range, &fences))
        );
        assert!(
            scan_images(text)
                .iter()
                .all(|image| !range_overlaps_any(&image.alt_text_range, &fences))
        );
        assert_eq!(scan_thematic_breaks(text).len(), 1);
        assert!(
            scan_thematic_breaks(text)
                .iter()
                .all(|r| !range_overlaps_any(r, &fences))
        );
    }

    #[test]
    fn tilde_and_unclosed_fences_are_excluded() {
        let tilde = "~~~\n==🔵 inside==\n~~~\n==outside==";
        let tilde_fences = fenced_code_block_ranges(tilde);
        assert_eq!(tilde_fences.len(), 1);
        assert!(!scan_conceals(tilde).is_empty());
        assert!(
            scan_conceals(tilde)
                .iter()
                .all(|c| !range_overlaps_any(&c.range, &tilde_fences))
        );

        // An unclosed fence excludes to end of buffer so in-progress code stays
        // literal while it is being typed.
        let unclosed = "==before==\n```\n==🔴 still typing==\n[x](y)";
        let unclosed_fences = fenced_code_block_ranges(unclosed);
        assert_eq!(unclosed_fences.len(), 1);
        assert_eq!(unclosed_fences[0].end, unclosed.len());
        assert!(
            scan_conceals(unclosed)
                .iter()
                .all(|c| !range_overlaps_any(&c.range, &unclosed_fences))
        );
    }

    #[test]
    fn backtick_fence_with_backtick_info_string_is_not_a_fence() {
        // CommonMark: a backtick opener whose info string carries a backtick is not a
        // fence, so the following YMD line still styles.
        let text = "```text with `inline`\n==🔵 styled==";
        assert!(fenced_code_block_ranges(text).is_empty());
        assert!(!scan_conceals(text).is_empty());

        // A tilde info string MAY contain backticks.
        let tilde = "~~~ `info`\n==🔵 hidden==\n~~~";
        assert_eq!(fenced_code_block_ranges(tilde).len(), 1);
    }

    #[test]
    fn four_space_indent_is_not_a_fence_so_indented_ymd_still_styles() {
        // 4+ leading spaces is an indented code block in CommonMark, which YMD does
        // NOT handle — only fenced blocks. So an indented ``` is not a fence opener
        // (the deliberate tree-sitter gap, #zed-09), and the following YMD line styles.
        let text = "    ```\n==🔵 styled==";
        assert!(fenced_code_block_ranges(text).is_empty());
        assert!(!scan_conceals(text).is_empty());

        // Up to three leading spaces is still a valid fence.
        let indented_fence = "   ```\n==🔵 hidden==\n   ```";
        assert_eq!(fenced_code_block_ranges(indented_fence).len(), 1);
    }

    #[test]
    fn scans_block_quote_lines() {
        let text = "> one\n>> two\n> > three\n>\nnormal";
        let block_quotes = scan_block_quotes(text);
        let content_fragments = block_quotes
            .iter()
            .map(|block_quote| &text[block_quote.content_range.clone()])
            .collect::<Vec<_>>();
        let line_fragments = block_quotes
            .iter()
            .map(|block_quote| &text[block_quote.line_range.clone()])
            .collect::<Vec<_>>();

        // A bare `>` is a quote line with empty content; `normal` is not a quote.
        assert_eq!(content_fragments, vec!["one", "two", "three", ""]);
        assert_eq!(line_fragments, vec!["> one", ">> two", "> > three", ">"]);
    }

    #[test]
    fn block_quote_allows_up_to_three_leading_spaces() {
        // Up to three leading spaces still opens a quote; four spaces is indented
        // code and is not recognized (mirrors the fence leading-space rule).
        let text = "   > indented\n    > code";
        let block_quotes = scan_block_quotes(text);
        let line_fragments = block_quotes
            .iter()
            .map(|block_quote| &text[block_quote.line_range.clone()])
            .collect::<Vec<_>>();

        assert_eq!(line_fragments, vec!["   > indented"]);
    }

    #[test]
    fn skips_block_quote_lines_inside_fenced_code_blocks() {
        // #zed-09 fence exclusion: a `>` line inside a code block stays raw.
        let text = "```md\n> not quote\n```\n> quote";
        let block_quotes = scan_block_quotes(text);
        let content_fragments = block_quotes
            .iter()
            .map(|block_quote| &text[block_quote.content_range.clone()])
            .collect::<Vec<_>>();

        assert_eq!(content_fragments, vec!["quote"]);
    }

    #[test]
    fn scans_task_checkboxes() {
        // Unchecked, checked, and an indented (nested) task: each range covers the
        // six marker bytes only, and the indentation before a nested marker is left
        // out of the range so it stays visible.
        assert_eq!(
            scan_checkboxes("- [ ] todo\n- [x] done\n  - [ ] nested"),
            vec![
                YmdCheckbox {
                    range: 0..6,
                    checked: false,
                },
                YmdCheckbox {
                    range: 11..17,
                    checked: true,
                },
                YmdCheckbox {
                    range: 24..30,
                    checked: false,
                },
            ]
        );
    }

    #[test]
    fn ignores_invalid_task_checkboxes() {
        // Dialect boundaries (walk Q5): no bullet, a marker mid-line, a missing
        // trailing space, an UPPERCASE `X`, and a non-dash bullet all stay raw.
        assert!(scan_checkboxes("[ ] missing list marker").is_empty());
        assert!(scan_checkboxes("text - [ ] middle").is_empty());
        assert!(scan_checkboxes("- [ ]missing trailing space").is_empty());
        assert!(scan_checkboxes("- [X] uppercase").is_empty());
        assert!(scan_checkboxes("* [ ] other marker").is_empty());
        assert!(scan_checkboxes("+ [ ] other marker").is_empty());
    }

    #[test]
    fn skips_task_checkboxes_inside_fenced_code_blocks() {
        // #zed-09 fence exclusion: a `- [ ]` inside a code block stays raw; only the
        // real task after the fence is recognized.
        assert_eq!(
            scan_checkboxes("```md\n- [ ] code\n```\n- [x] real"),
            vec![YmdCheckbox {
                range: 21..27,
                checked: true,
            }]
        );
    }
}
