use gpui::{HighlightStyle, Hsla, rgba};
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

fn for_each_line(text: &str, mut callback: impl FnMut(&str, usize)) {
    let mut line_start = 0;

    for line in text.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let line_content_end = line_end - usize::from(line.ends_with('\n'));
        callback(&text[line_start..line_content_end], line_start);
        line_start = line_end;
    }
}

pub(crate) fn scan(text: &str) -> Vec<YmdHighlight> {
    let mut highlights = Vec::new();

    for_each_line(text, |line_content, line_start| {
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

pub(crate) fn scan_conceals(text: &str) -> Vec<YmdConceal> {
    let mut conceals = Vec::new();

    for_each_line(text, |line_content, line_start| {
        let inline_highlights = scan_line_background_markups(line_content, line_start);
        for inline_highlight in &inline_highlights {
            conceals.push(YmdConceal {
                range: inline_highlight.open_marker_range.clone(),
            });
            conceals.push(YmdConceal {
                range: inline_highlight.close_marker_range.clone(),
            });
        }

        let heading_range = heading_conceal_range(line_content, line_start);
        if let Some(heading_range) = heading_range.clone() {
            conceals.push(YmdConceal {
                range: heading_range,
            });
        }

        let captures = capture_ranges(&inline_highlights, line_start);
        if let Some((emoji_range, _)) = first_effective_emoji(line_content, &captures) {
            let absolute_emoji_range =
                line_start + emoji_range.start..line_start + emoji_range.end;
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
        assert_eq!(
            scan_conceals("==🔴=="),
            vec![YmdConceal { range: 2..6 }]
        );
        assert_eq!(
            scan_conceals("==🔴 open"),
            vec![YmdConceal { range: 2..6 }]
        );
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
        assert_eq!(scan_conceals("#heading 🔵"), vec![YmdConceal { range: 9..13 }]);
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
}
