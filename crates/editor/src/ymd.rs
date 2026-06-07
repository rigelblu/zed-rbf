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

pub(crate) fn scan(text: &str) -> Vec<YmdHighlight> {
    let mut highlights = Vec::new();
    let mut line_start = 0;

    for line in text.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let line_content_end = line_end - usize::from(line.ends_with('\n'));
        let line_content = &text[line_start..line_content_end];

        let marker_ranges = scan_line_backgrounds(line_content, line_start, &mut highlights);

        if let Some(color) = line_foreground_color(line_content, &marker_ranges)
            && line_start < line_content_end
        {
            highlights.push(YmdHighlight {
                range: line_start..line_content_end,
                kind: YmdHighlightKind::LineForeground(color),
            });
        }

        line_start = line_end;
    }

    highlights
}

pub(crate) fn scan_conceals(text: &str) -> Vec<YmdConceal> {
    let mut conceals = Vec::new();
    let mut line_start = 0;

    for line in text.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let line_content_end = line_end - usize::from(line.ends_with('\n'));
        let line_content = &text[line_start..line_content_end];

        scan_line_conceals(line_content, line_start, &mut conceals);

        line_start = line_end;
    }

    conceals
}

// An emoji captured by a valid `==...==` highlight pair belongs to the background
// mechanism only; any emoji outside valid pairs is standalone and the first one
// colors the whole line's foreground.
fn line_foreground_color(line: &str, marker_ranges: &[Range<usize>]) -> Option<YmdColor> {
    let mut search_start = 0;

    while search_start < line.len() {
        let (relative_emoji_start, color, emoji_len) =
            YmdColor::first_match_in(&line[search_start..])?;
        let emoji_start = search_start + relative_emoji_start;

        if marker_ranges.iter().any(|range| range.contains(&emoji_start)) {
            search_start = emoji_start + emoji_len;
        } else {
            return Some(color);
        }
    }

    None
}

fn scan_line_backgrounds(
    line: &str,
    line_start: usize,
    highlights: &mut Vec<YmdHighlight>,
) -> Vec<Range<usize>> {
    scan_line_background_markups(line, line_start)
        .into_iter()
        .map(|inline_highlight| {
            highlights.push(YmdHighlight {
                range: inline_highlight.range,
                kind: YmdHighlightKind::Background(inline_highlight.color),
            });
            inline_highlight.open_marker_range.start - line_start
                ..inline_highlight.close_marker_range.end - line_start
        })
        .collect()
}

fn scan_line_conceals(line: &str, line_start: usize, conceals: &mut Vec<YmdConceal>) {
    for inline_highlight in scan_line_background_markups(line, line_start) {
        conceals.push(YmdConceal {
            range: inline_highlight.open_marker_range,
        });
        conceals.push(YmdConceal {
            range: inline_highlight.close_marker_range,
        });
    }
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
    fn ignores_empty_and_unterminated_highlights() {
        assert!(scan("====").is_empty());
        assert!(scan("==missing close").is_empty());
        assert!(scan_conceals("====").is_empty());
        assert!(scan_conceals("==missing close").is_empty());
        assert!(scan_conceals("==🔴==").is_empty());
        assert!(scan_conceals("==🔴 open").is_empty());
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
        assert!(scan_conceals("==🔵 starts\nends==").is_empty());
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
