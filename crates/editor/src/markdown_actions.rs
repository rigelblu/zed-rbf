use super::*;

impl Editor {
    pub fn toggle_markdown_heading(
        &mut self,
        action: &ToggleHeading,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let level = action.level.clamp(1, 6);
        self.manipulate_mutable_lines_in_markdown(window, cx, |lines| {
            for line in lines {
                *line = Cow::Owned(toggle_heading_line(line.as_ref(), level));
            }
        });
    }

    pub fn toggle_markdown_bold(
        &mut self,
        _: &ToggleBold,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_markdown_inline_format("**", window, cx);
    }

    pub fn toggle_markdown_italic(
        &mut self,
        _: &ToggleItalic,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_markdown_inline_format("*", window, cx);
    }

    pub fn toggle_markdown_bulleted_list(
        &mut self,
        _: &ToggleBulletedList,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_mutable_lines_in_markdown(window, cx, |lines| {
            let all_lines_list_items = lines.iter().all(|line| {
                let (_, rest) = split_indent(line);
                list_marker_len(rest).is_some()
            });

            for line in lines {
                *line = Cow::Owned(if all_lines_list_items {
                    remove_list_marker(line.as_ref())
                } else {
                    line_as_bullet(line.as_ref())
                });
            }
        });
    }

    pub fn toggle_markdown_task_list(
        &mut self,
        _: &ToggleTaskList,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_in_markdown_language(cx) || self.read_only(cx) {
            return;
        }

        let cursor_restoration_selections = self.task_toggle_cursor_restoration_selections(cx);

        self.manipulate_mutable_lines(window, cx, |lines| {
            let all_lines_task_items = lines.iter().all(|line| {
                let (_, rest) = split_indent(line);
                task_marker_len(rest).is_some()
            });

            for line in lines {
                *line = Cow::Owned(if all_lines_task_items {
                    toggle_task_marker(line.as_ref())
                } else {
                    line_as_task(line.as_ref())
                });
            }
        });

        if let Some(cursor_restoration_selections) = cursor_restoration_selections {
            self.change_selections(Default::default(), window, cx, |selections| {
                selections.select(cursor_restoration_selections);
            });
        }
    }

    pub fn toggle_markdown_block_quote(
        &mut self,
        _: &ToggleBlockQuote,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manipulate_mutable_lines_in_markdown(window, cx, |lines| {
            let all_lines_quoted = lines.iter().all(|line| line.starts_with('>'));

            for line in lines.iter_mut() {
                let stripped_line = match line.strip_prefix("> ").or_else(|| line.strip_prefix('>'))
                {
                    Some(rest) => rest.to_string(),
                    None => line.to_string(),
                };

                *line = if all_lines_quoted {
                    Cow::Owned(stripped_line)
                } else if stripped_line.trim().is_empty() {
                    Cow::Borrowed(">")
                } else {
                    Cow::Owned(format!("> {stripped_line}"))
                };
            }
        });
    }

    fn manipulate_mutable_lines_in_markdown<Fn>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        callback: Fn,
    ) where
        Fn: FnMut(&mut Vec<Cow<'_, str>>),
    {
        if !self.is_in_markdown_language(cx) {
            return;
        }

        self.manipulate_mutable_lines(window, cx, callback);
    }

    fn is_in_markdown_language(&self, cx: &mut App) -> bool {
        let snapshot = self.buffer.read(cx).snapshot(cx);
        let head = self
            .selections
            .newest::<MultiBufferOffset>(&self.display_snapshot(cx))
            .head();
        snapshot
            .language_at(head)
            .is_some_and(|language| language.name() == "Markdown")
    }

    fn task_toggle_cursor_restoration_selections(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Selection<MultiBufferOffset>>> {
        let display_snapshot = self.display_snapshot(cx);
        let offset_selections = self.selections.all::<MultiBufferOffset>(&display_snapshot);
        if offset_selections.is_empty()
            || !offset_selections
                .iter()
                .all(|selection| selection.is_empty())
        {
            return None;
        }

        let buffer = self.buffer.read(cx).snapshot(cx);
        let point_selections = self.selections.all::<Point>(&display_snapshot);
        point_selections
            .iter()
            .all(|selection| line_at_point_is_task(&buffer, selection.start))
            .then_some(offset_selections)
    }

    fn toggle_markdown_inline_format(
        &mut self,
        delimiter: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_in_markdown_language(cx) || self.read_only(cx) {
            return;
        }

        let buffer = self.buffer.read(cx).snapshot(cx);
        let selections = self.selections.all_adjusted(&self.display_snapshot(cx));
        let mut edits = Vec::new();
        let mut new_selections = Vec::new();
        let mut offset_delta = 0isize;

        for selection in selections {
            let mut content_range = if selection.is_empty() {
                let cursor_offset = buffer.point_to_offset(selection.start);
                buffer.surrounding_word(cursor_offset, None).0
            } else if selection.start <= selection.end {
                buffer.point_to_offset(selection.start)..buffer.point_to_offset(selection.end)
            } else {
                buffer.point_to_offset(selection.end)..buffer.point_to_offset(selection.start)
            };

            let mut selected_text = buffer
                .text_for_range(content_range.clone())
                .collect::<String>();
            if selection.is_empty() && selected_text.trim().is_empty() {
                let cursor_offset = buffer.point_to_offset(selection.start);
                content_range = cursor_offset..cursor_offset;
                selected_text.clear();
            }

            if selected_text.contains('\n') {
                new_selections.push(Selection {
                    id: selection.id,
                    start: offset_by_delta(buffer.point_to_offset(selection.start), offset_delta),
                    end: offset_by_delta(buffer.point_to_offset(selection.end), offset_delta),
                    goal: SelectionGoal::None,
                    reversed: selection.reversed,
                });
                continue;
            }

            let edit = inline_format_edit(&buffer, content_range, &selected_text, delimiter);
            let adjusted_start =
                (edit.range.start.0 as isize + offset_delta + edit.selection_start_offset as isize)
                    .max(0) as usize;
            let adjusted_end = adjusted_start + edit.selection_len;
            let old_len = edit.range.end.0 - edit.range.start.0;
            let new_len = edit.replacement.len();

            new_selections.push(Selection {
                id: selection.id,
                start: MultiBufferOffset(adjusted_start),
                end: MultiBufferOffset(adjusted_end),
                goal: SelectionGoal::None,
                reversed: selection.reversed,
            });
            offset_delta += new_len as isize - old_len as isize;
            edits.push((edit.range, edit.replacement));
        }

        if edits.is_empty() {
            return;
        }

        self.transact(window, cx, |this, window, cx| {
            this.buffer.update(cx, |buffer, cx| {
                buffer.edit(edits, None, cx);
            });

            this.change_selections(Default::default(), window, cx, |selections| {
                selections.select(new_selections);
            });
            this.request_autoscroll(Autoscroll::fit(), cx);
        });
    }
}

struct InlineFormatEdit {
    range: Range<MultiBufferOffset>,
    replacement: String,
    selection_start_offset: usize,
    selection_len: usize,
}

fn inline_format_edit(
    buffer: &MultiBufferSnapshot,
    content_range: Range<MultiBufferOffset>,
    selected_text: &str,
    delimiter: &str,
) -> InlineFormatEdit {
    if let Some(stripped_text) = strip_selected_delimiters(selected_text, delimiter) {
        return InlineFormatEdit {
            range: content_range,
            selection_start_offset: 0,
            selection_len: stripped_text.len(),
            replacement: stripped_text,
        };
    }

    if let Some(delimited_range) =
        surrounding_delimiter_range(buffer, content_range.clone(), delimiter)
    {
        return InlineFormatEdit {
            range: delimited_range,
            replacement: selected_text.to_string(),
            selection_start_offset: 0,
            selection_len: selected_text.len(),
        };
    }

    let replacement = format!("{delimiter}{selected_text}{delimiter}");
    let selection_start_offset = if selected_text.is_empty() {
        delimiter.len()
    } else {
        0
    };
    let selection_len = if selected_text.is_empty() {
        0
    } else {
        replacement.len()
    };
    InlineFormatEdit {
        range: content_range,
        selection_start_offset,
        selection_len,
        replacement,
    }
}

fn offset_by_delta(offset: MultiBufferOffset, delta: isize) -> MultiBufferOffset {
    MultiBufferOffset((offset.0 as isize + delta).max(0) as usize)
}

fn strip_selected_delimiters(text: &str, delimiter: &str) -> Option<String> {
    let minimum_len = delimiter.len() * 2;
    if text.len() < minimum_len || !text.starts_with(delimiter) || !text.ends_with(delimiter) {
        return None;
    }

    if delimiter.len() == 1 {
        let doubled = format!("{delimiter}{delimiter}");
        if text.starts_with(&doubled) || text.ends_with(&doubled) {
            return None;
        }
    }

    Some(text[delimiter.len()..text.len() - delimiter.len()].to_string())
}

fn surrounding_delimiter_range(
    buffer: &MultiBufferSnapshot,
    content_range: Range<MultiBufferOffset>,
    delimiter: &str,
) -> Option<Range<MultiBufferOffset>> {
    let leading_start = delimiter_start_before(buffer, content_range.start, delimiter)?;
    let trailing_end = delimiter_end_after(buffer, content_range.end, delimiter)?;

    if delimiter.len() == 1
        && delimiter_repeated_at_either_edge(buffer, leading_start, trailing_end)
    {
        return None;
    }

    Some(leading_start..trailing_end)
}

fn delimiter_start_before(
    buffer: &MultiBufferSnapshot,
    offset: MultiBufferOffset,
    delimiter: &str,
) -> Option<MultiBufferOffset> {
    let mut start = offset;
    for expected in delimiter.chars().rev() {
        if buffer.reversed_chars_at(start).next()? != expected {
            return None;
        }
        start = MultiBufferOffset(start.0.checked_sub(expected.len_utf8())?);
    }
    Some(start)
}

fn delimiter_end_after(
    buffer: &MultiBufferSnapshot,
    offset: MultiBufferOffset,
    delimiter: &str,
) -> Option<MultiBufferOffset> {
    let mut end = offset;
    for expected in delimiter.chars() {
        if buffer.chars_at(end).next()? != expected {
            return None;
        }
        end = MultiBufferOffset(end.0.checked_add(expected.len_utf8())?);
    }
    Some(end)
}

fn delimiter_repeated_at_either_edge(
    buffer: &MultiBufferSnapshot,
    leading_start: MultiBufferOffset,
    trailing_end: MultiBufferOffset,
) -> bool {
    buffer.reversed_chars_at(leading_start).next() == Some('*')
        || buffer.chars_at(trailing_end).next() == Some('*')
}

fn toggle_heading_line(line: &str, level: u8) -> String {
    let (indent, rest) = split_indent(line);
    let prefix = format!("{} ", "#".repeat(level as usize));
    match heading_marker(rest) {
        Some((current_level, marker_len)) if current_level == level => {
            format!("{indent}{}", &rest[marker_len..])
        }
        Some((_, marker_len)) => format!("{indent}{prefix}{}", &rest[marker_len..]),
        None => format!("{indent}{prefix}{rest}"),
    }
}

fn heading_marker(text: &str) -> Option<(u8, usize)> {
    let bytes = text.as_bytes();
    let level = bytes.iter().take_while(|byte| **byte == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }

    if level == bytes.len() {
        return Some((level as u8, level));
    }

    if !bytes[level].is_ascii_whitespace() {
        return None;
    }

    let marker_len = bytes[level..]
        .iter()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count()
        + level;
    Some((level as u8, marker_len))
}

fn line_as_bullet(line: &str) -> String {
    let (indent, rest) = split_indent(line);
    if let Some(marker_len) = task_marker_len(rest) {
        format!("{indent}- {}", &rest[marker_len..])
    } else if bullet_marker_len(rest).is_some() {
        line.to_string()
    } else {
        format!("{indent}- {rest}")
    }
}

fn line_as_task(line: &str) -> String {
    let (indent, rest) = split_indent(line);
    if task_marker_len(rest).is_some() {
        line.to_string()
    } else if let Some(marker_len) = bullet_marker_len(rest) {
        format!("{indent}- [ ] {}", &rest[marker_len..])
    } else {
        format!("{indent}- [ ] {rest}")
    }
}

fn remove_list_marker(line: &str) -> String {
    let (indent, rest) = split_indent(line);
    if let Some(marker_len) = task_marker_len(rest).or_else(|| bullet_marker_len(rest)) {
        format!("{indent}{}", &rest[marker_len..])
    } else {
        line.to_string()
    }
}

fn toggle_task_marker(line: &str) -> String {
    let (indent, rest) = split_indent(line);
    if task_marker_len(rest).is_some() {
        let checked = rest
            .as_bytes()
            .get(3)
            .is_some_and(|byte| matches!(byte, b'x' | b'X'));
        let marker = if checked { "- [ ]" } else { "- [x]" };
        format!("{indent}{marker}{}", &rest[5..])
    } else {
        line.to_string()
    }
}

fn line_at_point_is_task(buffer: &MultiBufferSnapshot, point: Point) -> bool {
    let row = MultiBufferRow(point.row);
    let line = buffer
        .text_for_range(Point::new(point.row, 0)..Point::new(point.row, buffer.line_len(row)))
        .collect::<String>();
    let (_, rest) = split_indent(&line);
    task_marker_len(rest).is_some()
}

fn list_marker_len(line: &str) -> Option<usize> {
    task_marker_len(line).or_else(|| bullet_marker_len(line))
}

fn bullet_marker_len(line: &str) -> Option<usize> {
    if line == "-" {
        Some(1)
    } else if line.starts_with("- ") {
        Some(2)
    } else {
        None
    }
}

fn task_marker_len(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    if bytes.len() < 5
        || bytes[0] != b'-'
        || bytes[1] != b' '
        || bytes[2] != b'['
        || !matches!(bytes[3], b' ' | b'x' | b'X')
        || bytes[4] != b']'
    {
        return None;
    }

    if bytes.len() == 5 {
        return Some(5);
    }

    bytes[5].is_ascii_whitespace().then_some(6)
}

fn split_indent(line: &str) -> (&str, &str) {
    let rest_start = line
        .char_indices()
        .find_map(|(index, character)| (character != ' ' && character != '\t').then_some(index))
        .unwrap_or(line.len());
    (&line[..rest_start], &line[rest_start..])
}
