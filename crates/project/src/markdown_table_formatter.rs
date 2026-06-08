use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Alignment {
    Left,
    Right,
    Center,
    None,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TableRow {
    leading_whitespace: String,
    cells: Vec<String>,
}

pub fn align_markdown_tables(text: &str) -> Option<String> {
    let has_final_newline = text.ends_with('\n');
    let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines = text.split('\n').collect::<Vec<_>>();
    if has_final_newline {
        lines.pop();
    }
    let lines = lines
        .into_iter()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>();

    let fenced_code_lines = fenced_code_lines(&lines);
    let mut output = lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let mut changed = false;
    let mut line_index = 0;

    while line_index + 1 < lines.len() {
        if fenced_code_lines[line_index] {
            line_index += 1;
            continue;
        }

        let Some(header) = parse_table_row(lines[line_index]) else {
            line_index += 1;
            continue;
        };
        let Some(delimiter) = parse_table_row(lines[line_index + 1]) else {
            line_index += 1;
            continue;
        };
        if !delimiter
            .cells
            .iter()
            .all(|cell| parse_alignment(cell).is_some())
        {
            line_index += 1;
            continue;
        }

        let mut table_rows = vec![header, delimiter];
        let start_line = line_index;
        line_index += 2;

        let mut table_is_parseable = true;
        while line_index < lines.len() && !fenced_code_lines[line_index] {
            let row = match parse_table_row(lines[line_index]) {
                Some(row) => row,
                None => {
                    if looks_like_table_row(lines[line_index]) {
                        table_is_parseable = false;
                    }
                    break;
                }
            };
            table_rows.push(row);
            line_index += 1;
        }

        if !table_is_parseable {
            continue;
        }

        let aligned_rows = align_table_rows(&table_rows);
        for (row_index, aligned_row) in aligned_rows.into_iter().enumerate() {
            let output_index = start_line + row_index;
            if output[output_index] != aligned_row {
                changed = true;
                output[output_index] = aligned_row;
            }
        }
    }

    changed.then(|| {
        let mut new_text = output.join(line_ending);
        if has_final_newline {
            new_text.push_str(line_ending);
        }
        new_text
    })
}

fn looks_like_table_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

fn align_table_rows(rows: &[TableRow]) -> Vec<String> {
    let column_count = rows
        .iter()
        .map(|row| row.cells.len())
        .max()
        .unwrap_or_default();
    let alignments = (0..column_count)
        .map(|column_index| {
            rows.get(1)
                .and_then(|row| row.cells.get(column_index))
                .and_then(|cell| parse_alignment(cell))
                .unwrap_or(Alignment::None)
        })
        .collect::<Vec<_>>();
    let column_widths = (0..column_count)
        .map(|column_index| {
            rows.iter()
                .enumerate()
                .filter(|(row_index, _)| *row_index != 1)
                .map(|(_, row)| {
                    row.cells
                        .get(column_index)
                        .map(|cell| UnicodeWidthStr::width(cell.trim()))
                        .unwrap_or_default()
                })
                .max()
                .unwrap_or_default()
                .max(3)
        })
        .collect::<Vec<_>>();

    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            let rendered_cells = (0..column_count)
                .map(|column_index| {
                    if row_index == 1 {
                        render_delimiter_cell(column_widths[column_index], alignments[column_index])
                    } else {
                        let content = row
                            .cells
                            .get(column_index)
                            .map(|cell| cell.trim())
                            .unwrap_or_default();
                        render_content_cell(
                            content,
                            column_widths[column_index],
                            alignments[column_index],
                        )
                    }
                })
                .collect::<Vec<_>>();
            format!(
                "{}| {} |",
                row.leading_whitespace,
                rendered_cells.join(" | ")
            )
        })
        .collect()
}

fn render_content_cell(content: &str, width: usize, alignment: Alignment) -> String {
    let padding = width.saturating_sub(UnicodeWidthStr::width(content));
    match alignment {
        Alignment::Right => format!("{}{content}", " ".repeat(padding)),
        Alignment::Center => {
            let left_padding = padding / 2;
            let right_padding = padding - left_padding;
            format!(
                "{}{content}{}",
                " ".repeat(left_padding),
                " ".repeat(right_padding)
            )
        }
        Alignment::Left | Alignment::None => format!("{content}{}", " ".repeat(padding)),
    }
}

fn render_delimiter_cell(width: usize, alignment: Alignment) -> String {
    match alignment {
        Alignment::Left => format!(":{}", "-".repeat(width.saturating_sub(1))),
        Alignment::Right => format!("{}:", "-".repeat(width.saturating_sub(1))),
        Alignment::Center => format!(":{}:", "-".repeat(width.saturating_sub(2))),
        Alignment::None => "-".repeat(width),
    }
}

fn parse_alignment(cell: &str) -> Option<Alignment> {
    let marker = cell
        .trim()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    if marker.is_empty()
        || !marker.contains('-')
        || marker
            .chars()
            .any(|character| character != '-' && character != ':')
    {
        return None;
    }

    let starts_with_colon = marker.starts_with(':');
    let ends_with_colon = marker.ends_with(':');
    Some(match (starts_with_colon, ends_with_colon) {
        (true, true) => Alignment::Center,
        (true, false) => Alignment::Left,
        (false, true) => Alignment::Right,
        (false, false) => Alignment::None,
    })
}

fn parse_table_row(line: &str) -> Option<TableRow> {
    let leading_whitespace_len = line
        .char_indices()
        .find_map(|(index, character)| (!character.is_whitespace()).then_some(index))
        .unwrap_or(line.len());
    let leading_whitespace = &line[..leading_whitespace_len];
    let content = line[leading_whitespace_len..].trim_end();
    if !content.starts_with('|') {
        return None;
    }

    let mut cells = Vec::new();
    let mut cell_start = 1;
    let mut escaped = false;
    let mut code_span_run_len = None;
    let mut skip_until = 0;
    let mut found_closing_pipe = false;

    for (index, character) in content.char_indices().skip(1) {
        if index < skip_until {
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => {
                escaped = true;
            }
            '`' => {
                let run_len = backtick_run_len(content.as_bytes(), index);
                if code_span_run_len == Some(run_len) {
                    code_span_run_len = None;
                } else if code_span_run_len.is_none() {
                    code_span_run_len = Some(run_len);
                }
                skip_until = index + run_len;
            }
            '|' if code_span_run_len.is_none() => {
                cells.push(content[cell_start..index].to_string());
                cell_start = index + character.len_utf8();
                found_closing_pipe = true;
            }
            _ => {}
        }
    }

    if code_span_run_len.is_some() {
        return None;
    }
    if !found_closing_pipe || !content[cell_start..].trim().is_empty() {
        return None;
    }

    Some(TableRow {
        leading_whitespace: leading_whitespace.to_string(),
        cells,
    })
}

fn backtick_run_len(bytes: &[u8], start: usize) -> usize {
    let mut len = 0;
    while start + len < bytes.len() && bytes[start + len] == b'`' {
        len += 1;
    }
    len
}

fn fenced_code_lines(lines: &[&str]) -> Vec<bool> {
    let mut result = vec![false; lines.len()];
    let mut active_fence: Option<char> = None;

    for (line_index, line) in lines.iter().enumerate() {
        let fence = fence_marker(line);
        if let Some(marker) = active_fence {
            result[line_index] = true;
            if fence == Some(marker) {
                active_fence = None;
            }
        } else if let Some(marker) = fence {
            result[line_index] = true;
            active_fence = Some(marker);
        }
    }

    result
}

fn fence_marker(line: &str) -> Option<char> {
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();
    if indent_len > 3 {
        return None;
    }

    let mut characters = trimmed.chars();
    let marker = characters.next()?;
    if marker != '`' && marker != '~' {
        return None;
    }

    let marker_count = 1 + characters
        .take_while(|character| *character == marker)
        .count();
    (marker_count >= 3).then_some(marker)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_multi_backtick_code_spans_as_single_cells() {
        let input = "| Code | Status |\n| --- | --- |\n| ``cmd | grep`` | ok |\n";
        let expected = "| Code           | Status |\n| -------------- | ------ |\n| ``cmd | grep`` | ok     |\n";

        let output = align_markdown_tables(input).expect("table should align");
        assert_eq!(output, expected);
        assert_eq!(align_markdown_tables(&output), None);
    }

    #[test]
    fn skips_tables_with_unbalanced_backtick_runs() {
        let input = "| Code | Status |\n| --- | --- |\n| `cmd | grep | ok |\n";

        assert_eq!(align_markdown_tables(input), None);
    }

    #[test]
    fn aligns_cjk_and_emoji_by_display_width() {
        let input = "| Status | Item |\n| --- | --- |\n| ✅ | verified |\n| 語 | text |\n";
        let expected = "| Status | Item     |\n| ------ | -------- |\n| ✅     | verified |\n| 語     | text     |\n";

        let output = align_markdown_tables(input).expect("table should align");
        assert_eq!(output, expected);
        assert_eq!(align_markdown_tables(&output), None);
    }

    #[test]
    fn preserves_crlf_line_endings() {
        let input = "| A | BB |\r\n| --- | --- |\r\n| longer | c |\r\n";
        let expected = "| A      | BB  |\r\n| ------ | --- |\r\n| longer | c   |\r\n";

        let output = align_markdown_tables(input).expect("table should align");
        assert_eq!(output, expected);
        assert_eq!(align_markdown_tables(&output), None);
    }
}
