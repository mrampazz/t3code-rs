//! A command row's output as the web folds it (`@t3tools/shared/terminalOutput`): the last 64
//! lines stay cursor-addressable (carriage returns, cursor moves, erase), older lines settle into
//! text, and the whole is a bounded tail whose only escapes are canonical SGR pens. The server
//! folds the same way, so a `replace` frame and a chain of `append`s render identically.

use unicode_width::UnicodeWidthChar;

/// `TERMINAL_OUTPUT_TAIL_CHARS`, in UTF-16 units like the web.
pub(crate) const TAIL_CHARS: usize = 64 * 1024;
const LIVE_LINES: usize = 64;
const MAX_PENDING_ESCAPE: usize = 64;
const MAX_LINE_RUNS: usize = 2_048;
const ESC: char = '\u{1b}';

#[derive(Clone, Debug, PartialEq)]
struct Run {
    pen: String,
    text: String,
}

type Line = Vec<Run>;

#[derive(Clone, Debug)]
struct Screen {
    settled: (String, String),
    lines: Vec<Line>,
    row: usize,
    col: usize,
    pen: String,
    saved: Option<(usize, usize)>,
}

/// `TerminalOutputState`.
#[derive(Clone, Debug)]
pub(crate) struct TerminalOutput {
    /// Settled lines then the live window; its only escapes are SGR pens.
    pub text: String,
    pub truncated: bool,
    pending: String,
    screen: Screen,
}

impl Default for TerminalOutput {
    fn default() -> Self {
        Self {
            text: String::new(),
            truncated: false,
            pending: String::new(),
            screen: Screen {
                settled: (String::new(), String::new()),
                lines: vec![Vec::new()],
                row: 0,
                col: 0,
                pen: String::new(),
                saved: None,
            },
        }
    }
}

fn units(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

fn text_width(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.chars().map(char_width).sum()
    }
}

fn line_width(line: &Line) -> usize {
    line.iter().map(|r| text_width(&r.text)).sum()
}

fn line_units(line: &Line) -> usize {
    line.iter().map(|r| units(&r.text)).sum()
}

fn sgr(pen: &str) -> String {
    if pen.is_empty() {
        format!("{ESC}[0m")
    } else {
        format!("{ESC}[0;{pen}m")
    }
}

fn push_run(runs: &mut Line, pen: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    match runs.last_mut() {
        Some(last) if last.pen == pen => last.text.push_str(text),
        _ => runs.push(Run {
            pen: pen.to_owned(),
            text: text.to_owned(),
        }),
    }
}

/// The columns `start..end` of a line; overwriting half a wide glyph blanks the other half.
fn slice_line(line: &Line, start: usize, end: usize) -> Line {
    let mut out = Vec::new();
    let (mut col, mut base, mut base_end) = (0usize, 0usize, 0usize);
    for run in line {
        let mut text = String::new();
        for c in run.text.chars() {
            let width = char_width(c);
            if width > 0 {
                base = col;
                base_end = col + width;
            }
            if width == 0 {
                if base >= start && base < end && base_end <= end {
                    text.push(c);
                }
            } else if col < end && col + width > start {
                if col >= start && col + width <= end {
                    text.push(c);
                } else {
                    let blank = end.min(col + width) - start.max(col);
                    text.push_str(&" ".repeat(blank));
                }
            }
            col += width;
        }
        push_run(&mut out, &run.pen, &text);
    }
    out
}

/// Drops `count` UTF-16 units from a line's start; returns the columns removed.
fn trim_line_start(line: &mut Line, count: usize) -> usize {
    let mut removed = 0;
    let mut remaining = count;
    while let Some(run) = line.first_mut() {
        let mut cut = 0; // bytes
        let mut taken = 0; // units
        let mut chars = run.text.char_indices().peekable();
        while let Some((i, c)) = chars.peek().copied() {
            if taken >= remaining && char_width(c) != 0 {
                break;
            }
            taken += c.len_utf16();
            cut = i + c.len_utf8();
            chars.next();
        }
        removed += text_width(&run.text[..cut]);
        remaining = remaining.saturating_sub(taken);
        if cut < run.text.len() {
            run.text.drain(..cut);
            break;
        }
        line.remove(0);
    }
    removed
}

fn write_line(line: &Line, col: usize, text: &str, pen: &str) -> Line {
    let length = line_width(line);
    if col == length {
        let mut runs = line.clone();
        push_run(&mut runs, pen, text);
        return runs;
    }
    let mut runs = Vec::new();
    for run in slice_line(line, 0, col.min(length)) {
        push_run(&mut runs, &run.pen, &run.text);
    }
    if col > length {
        push_run(&mut runs, "", &" ".repeat(col - length));
    }
    push_run(&mut runs, pen, text);
    for run in slice_line(line, col + text_width(text), length) {
        push_run(&mut runs, &run.pen, &run.text);
    }
    runs
}

/// Erase in line: 0 cursor to end, 1 start to cursor, 2 whole line.
fn erase_line(line: &Line, col: usize, mode: usize) -> Line {
    let length = line_width(line);
    match mode {
        0 => slice_line(line, 0, col),
        1 => {
            let mut runs = Vec::new();
            push_run(&mut runs, "", &" ".repeat((col + 1).min(length)));
            for run in slice_line(line, col + 1, length) {
                push_run(&mut runs, &run.pen, &run.text);
            }
            runs
        }
        2 => Vec::new(),
        _ => line.clone(),
    }
}

fn serialize_line(line: &Line) -> String {
    let mut out = String::new();
    let mut current = "";
    for run in line {
        if run.pen != current {
            out.push_str(&sgr(&run.pen));
            current = &run.pen;
        }
        out.push_str(&run.text);
    }
    if !current.is_empty() {
        out.push_str(&sgr(""));
    }
    out
}

/// SGR attributes; `fg` / `bg` keep their codes (`31`, `38;5;208`, `48;2;1;2;3`).
#[derive(Default, Clone)]
pub(crate) struct Attributes {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub inverse: bool,
    pub fg: String,
    pub bg: String,
}

pub(crate) fn parse_pen(pen: &str) -> Attributes {
    let mut attributes = Attributes::default();
    if !pen.is_empty() {
        let params: Vec<i64> = pen.split(';').map(|p| p.parse().unwrap_or(-1)).collect();
        apply_sgr(&mut attributes, &params);
    }
    attributes
}

fn format_pen(a: &Attributes) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for (on, code) in [
        (a.bold, "1"),
        (a.dim, "2"),
        (a.italic, "3"),
        (a.underline, "4"),
        (a.inverse, "7"),
        (a.strike, "9"),
    ] {
        if on {
            parts.push(code);
        }
    }
    if !a.fg.is_empty() {
        parts.push(&a.fg);
    }
    if !a.bg.is_empty() {
        parts.push(&a.bg);
    }
    parts.join(";")
}

fn extended_color(code: i64, params: &[i64], index: usize) -> (Option<String>, usize) {
    match params.get(index + 1) {
        Some(5) => match params.get(index + 2) {
            Some(&v) if (0..=255).contains(&v) => (Some(format!("{code};5;{v}")), index + 3),
            _ => (None, index + 3),
        },
        Some(2) => match (
            params.get(index + 2),
            params.get(index + 3),
            params.get(index + 4),
        ) {
            (Some(r), Some(g), Some(b)) => (
                Some(format!("{code};2;{};{};{}", r & 255, g & 255, b & 255)),
                index + 5,
            ),
            _ => (None, index + 5),
        },
        _ => (None, index + 2),
    }
}

fn apply_sgr(a: &mut Attributes, params: &[i64]) {
    let params: &[i64] = if params.is_empty() { &[0] } else { params };
    let mut i = 0;
    while i < params.len() {
        let code = params[i];
        match code {
            0 | -1 => *a = Attributes::default(),
            1 => a.bold = true,
            2 => a.dim = true,
            3 => a.italic = true,
            4 => a.underline = true,
            7 => a.inverse = true,
            9 => a.strike = true,
            22 => (a.bold, a.dim) = (false, false),
            23 => a.italic = false,
            24 => a.underline = false,
            27 => a.inverse = false,
            29 => a.strike = false,
            30..=37 | 90..=97 => a.fg = code.to_string(),
            40..=47 | 100..=107 => a.bg = code.to_string(),
            39 => a.fg.clear(),
            49 => a.bg.clear(),
            38 | 48 | 58 => {
                // 58 (underline colour) is consumed but not kept: nothing renders it.
                let (color, next) = extended_color(code, params, i);
                if let Some(color) = color {
                    match code {
                        38 => a.fg = color,
                        48 => a.bg = color,
                        _ => {}
                    }
                }
                i = next;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
}

/// `csiParams`: colon sub-parameters folded into the semicolon form.
fn csi_params(params: &str) -> Vec<i64> {
    enum Pending {
        Code,
        Mode,
        Values(usize),
    }
    let mut numbers = Vec::new();
    let mut pending = Pending::Code;
    for part in params.split(';') {
        let mut sub: Vec<i64> = part.split(':').map(|p| p.parse().unwrap_or(-1)).collect();
        let head = sub[0];
        match pending {
            Pending::Values(n) => {
                numbers.push(head);
                pending = if n > 1 {
                    Pending::Values(n - 1)
                } else {
                    Pending::Code
                };
                continue;
            }
            Pending::Mode => {
                pending = Pending::Code;
                if sub.len() == 1 {
                    numbers.push(head);
                    if head == 5 {
                        pending = Pending::Values(1);
                    } else if head == 2 {
                        pending = Pending::Values(3);
                    }
                    continue;
                }
                if head == 2 && sub.len() >= 5 {
                    sub.remove(1);
                }
                let take = match head {
                    5 => 2,
                    2 => 4,
                    _ => 1,
                };
                numbers.extend(sub.into_iter().take(take));
                continue;
            }
            Pending::Code => {}
        }
        if sub.len() == 1 {
            numbers.push(head);
            if matches!(head, 38 | 48 | 58) {
                pending = Pending::Mode;
            }
            continue;
        }
        if !matches!(head, 38 | 48 | 58) {
            numbers.push(if head == 4 && sub[1] == 0 { 24 } else { head });
            continue;
        }
        if sub[1] == 2 && sub.len() >= 6 {
            sub.remove(2);
        }
        let take = match sub[1] {
            5 => 3,
            2 => 5,
            _ => 2,
        };
        numbers.extend(sub.into_iter().take(take));
    }
    numbers
}

/// The length in chars of a complete escape at `chars[index]` (an ESC), or `None` if unfinished.
fn escape_length(chars: &[char], index: usize) -> Option<usize> {
    let next = *chars.get(index + 1)?;
    if next == ESC {
        return Some(1);
    }
    if next == '[' {
        return (index + 2..chars.len())
            .find(|&k| ('\u{40}'..='\u{7e}').contains(&chars[k]))
            .map(|k| k - index + 1);
    }
    if [']', 'P', 'X', '_', '^'].contains(&next) {
        for k in index + 2..chars.len() {
            match chars[k] {
                '\u{7}' => return Some(k - index + 1),
                '\n' => return Some(k - index),
                ESC => {
                    let after = *chars.get(k + 1)?;
                    return Some(if after == '\\' {
                        k - index + 2
                    } else {
                        k - index
                    });
                }
                _ => {}
            }
        }
        return None;
    }
    let mut k = index + 1;
    while k < chars.len() && ('\u{20}'..='\u{2f}').contains(&chars[k]) {
        k += 1;
    }
    if k >= chars.len() {
        return None;
    }
    if k > index + 1 && !('\u{30}'..='\u{7e}').contains(&chars[k]) {
        return Some(k - index);
    }
    Some(k - index + 1)
}

struct Terminal {
    settled_head: String,
    settled_tail: String,
    lines: Vec<Line>,
    row: usize,
    col: usize,
    pen: String,
    saved: Option<(usize, usize)>,
    dropped: bool,
    max_chars: usize,
}

impl Terminal {
    fn write(&mut self, text: &str) {
        let mut line = write_line(&self.lines[self.row], self.col, text, &self.pen);
        self.col += text_width(text);
        if line.len() > MAX_LINE_RUNS {
            // Flatten old runs so later trimming stays cheap.
            let keep = line.len() - MAX_LINE_RUNS / 8;
            let plain: String = line[..keep].iter().map(|r| r.text.as_str()).collect();
            let mut flat = vec![Run {
                pen: String::new(),
                text: plain,
            }];
            flat.extend(line.drain(keep..));
            line = flat;
        }
        let current = line_units(&line);
        if current > self.max_chars {
            let removed = trim_line_start(&mut line, current - self.max_chars);
            self.dropped = true;
            self.col = self.col.saturating_sub(removed);
            if let Some((row, col)) = self.saved
                && row == self.row
            {
                self.saved = Some((row, col.saturating_sub(removed)));
            }
        }
        self.lines[self.row] = line;
    }

    fn newline(&mut self) {
        self.row += 1;
        self.col = 0;
        if self.row == self.lines.len() {
            self.lines.push(Vec::new());
        }
        while self.lines.len() > LIVE_LINES {
            self.scroll_out();
        }
    }

    fn scroll_out(&mut self) -> usize {
        let line = self.lines.remove(0);
        self.settle(&line);
        self.row = self.row.saturating_sub(1);
        if let Some((row, col)) = self.saved {
            self.saved = Some((row.saturating_sub(1), col));
        }
        line_units(&line)
    }

    fn settle(&mut self, line: &Line) {
        self.settled_tail.push_str(&serialize_line(line));
        self.settled_tail.push('\n');
        if units(&self.settled_head) + units(&self.settled_tail) > 2 * self.max_chars {
            let (parts, _) = tail_parts(
                vec![
                    std::mem::take(&mut self.settled_head),
                    std::mem::take(&mut self.settled_tail),
                ],
                self.max_chars,
            );
            let mut parts = parts.into_iter();
            self.settled_head = parts.next().unwrap_or_default();
            self.settled_tail = parts.next().unwrap_or_default();
            self.dropped = true;
        }
    }

    fn move_to(&mut self, row: isize, col: isize) {
        self.row = row.clamp(0, self.lines.len() as isize - 1) as usize;
        self.col = col.clamp(0, 2 * TAIL_CHARS as isize) as usize;
    }

    fn csi(&mut self, params: &str, final_char: char) {
        if !params
            .chars()
            .all(|c| c.is_ascii_digit() || c == ';' || c == ':')
            || (final_char != 'm' && params.contains(':'))
        {
            return;
        }
        if final_char == 'm' {
            let mut attributes = parse_pen(&self.pen);
            let codes = if params.is_empty() {
                Vec::new()
            } else {
                csi_params(params)
            };
            apply_sgr(&mut attributes, &codes);
            self.pen = format_pen(&attributes);
            return;
        }
        let numbers = if params.is_empty() {
            Vec::new()
        } else {
            csi_params(params)
        };
        let first = numbers.first().copied().unwrap_or(0).max(0);
        let count = first.max(1) as isize;
        let (row, col) = (self.row as isize, self.col as isize);
        match final_char {
            'A' => self.move_to(row - count, col),
            'B' => self.move_to(row + count, col),
            'C' => self.move_to(row, col + count),
            'D' => self.move_to(row, col - count),
            'E' => self.move_to(row + count, 0),
            'F' => self.move_to(row - count, 0),
            'G' => self.move_to(row, count - 1),
            // Rows count from the top of the window: there is no fixed screen height.
            'H' | 'f' => {
                let column = numbers.get(1).copied().unwrap_or(1).max(1) as isize;
                self.move_to(count - 1, column - 1)
            }
            'K' => {
                self.lines[self.row] = erase_line(&self.lines[self.row], self.col, first as usize)
            }
            'J' => match first {
                0 => {
                    self.lines[self.row] = erase_line(&self.lines[self.row], self.col, 0);
                    self.lines.truncate(self.row + 1);
                }
                1 => {
                    for line in &mut self.lines[..self.row] {
                        line.clear();
                    }
                    self.lines[self.row] = erase_line(&self.lines[self.row], self.col, 1);
                }
                3 => {
                    if !self.settled_head.is_empty() || !self.settled_tail.is_empty() {
                        self.dropped = true;
                    }
                    self.settled_head.clear();
                    self.settled_tail.clear();
                }
                2 => {
                    self.lines = vec![Vec::new()];
                    self.row = 0;
                    self.col = 0;
                }
                _ => {}
            },
            's' => self.saved = Some((self.row, self.col)),
            'u' => {
                if let Some((r, c)) = self.saved {
                    self.move_to(r as isize, c as isize)
                }
            }
            _ => {}
        }
    }

    fn escape(&mut self, sequence: &[char]) {
        match sequence.get(1) {
            Some('[') => {
                let params: String = sequence[2..sequence.len() - 1].iter().collect();
                self.csi(&params, sequence[sequence.len() - 1]);
            }
            Some('7') if sequence.len() == 2 => self.saved = Some((self.row, self.col)),
            Some('8') if sequence.len() == 2 => {
                if let Some((r, c)) = self.saved {
                    self.move_to(r as isize, c as isize)
                }
            }
            Some('M') if sequence.len() == 2 => {
                self.move_to(self.row as isize - 1, self.col as isize)
            }
            _ => {}
        }
    }

    /// Applies `input`; returns an unfinished escape left at its end.
    fn feed(&mut self, input: &str) -> String {
        let chars: Vec<char> = input.chars().collect();
        let mut printable = 0;
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let code = c as u32;
            if code == 0x09 || (code >= 0x20 && code != 0x7f && !(0x80..=0x9f).contains(&code)) {
                i += 1;
                continue;
            }
            if i > printable {
                let text: String = chars[printable..i].iter().collect();
                self.write(&text);
            }
            printable = i + 1;
            match code {
                0x0a => self.newline(),
                0x0d => self.col = 0,
                0x08 => self.col = self.col.saturating_sub(1),
                0x1b => match escape_length(&chars, i) {
                    None => {
                        let fragment: String = chars[i..].iter().collect();
                        if fragment.chars().count() <= MAX_PENDING_ESCAPE {
                            return fragment;
                        }
                        // Keep the introducer (and a possible ST start) so a long string
                        // payload stays hidden until it ends.
                        let second = chars.get(i + 1).copied().unwrap_or(' ');
                        if [']', 'P', 'X', '_', '^'].contains(&second) {
                            let mut kept = format!("{ESC}{second}");
                            if chars.last() == Some(&ESC) {
                                kept.push(ESC);
                            }
                            return kept;
                        }
                        return String::new();
                    }
                    Some(length) => {
                        let sequence = chars[i..i + length].to_vec();
                        self.escape(&sequence);
                        i += length - 1;
                        printable = i + 1;
                    }
                },
                _ => {}
            }
            i += 1;
        }
        if chars.len() > printable {
            let text: String = chars[printable..].iter().collect();
            self.write(&text);
        }
        String::new()
    }
}

/// The SGR in force at byte `index` of `text`, and where a cut there may safely start.
fn style_at(text: &str, index: usize) -> (usize, String) {
    let line_start = text[..index].rfind('\n').map_or(0, |n| n + 1);
    let mut prefix = String::new();
    let mut cursor = line_start;
    while cursor < index {
        let Some(found) = text[cursor..].find("\u{1b}[").map(|f| f + cursor) else {
            break;
        };
        if found >= index {
            break;
        }
        let Some(end) = text[found..].find('m').map(|e| e + found) else {
            break;
        };
        if end >= index {
            return (end + 1, text[found..=end].to_owned());
        }
        prefix = text[found..=end].to_owned();
        cursor = end + 1;
    }
    if prefix == sgr("") {
        prefix.clear();
    }
    (index, prefix)
}

/// The byte index where the last `max` UTF-16 units of `text` begin.
fn unit_offset_from_end(text: &str, max: usize) -> usize {
    let mut taken = 0;
    for (i, c) in text.char_indices().rev() {
        taken += c.len_utf16();
        if taken > max {
            return i + c.len_utf8();
        }
    }
    0
}

/// `terminalOutputTail`: the last `max` units, preferring to start on a fresh line.
fn tail(text: &str, max: usize) -> (String, bool) {
    let total = units(text);
    if total <= max {
        return (text.to_owned(), false);
    }
    let start = unit_offset_from_end(text, max);
    if let Some(newline) = text[start..].find('\n')
        && units(&text[start..start + newline]) < max / 4
    {
        return (text[start + newline + 1..].to_owned(), true);
    }
    let (start, prefix) = style_at(text, start);
    (format!("{prefix}{}", &text[start..]), true)
}

/// `terminalOutputTailParts`: `tail` of the parts' concatenation, cut within one part when
/// a newline allows.
fn tail_parts(parts: Vec<String>, max: usize) -> (Vec<String>, bool) {
    let total: usize = parts.iter().map(|p| units(p)).sum();
    if total <= max {
        return (parts, false);
    }
    let start = total - max;
    let mut offset = 0;
    for (index, part) in parts.iter().enumerate() {
        let len = units(part);
        if offset + len > start {
            let from_units = start.saturating_sub(offset);
            let mut byte = 0;
            let mut seen = 0;
            for (i, c) in part.char_indices() {
                if seen >= from_units {
                    byte = i;
                    break;
                }
                seen += c.len_utf16();
                byte = i + c.len_utf8();
            }
            if let Some(newline) = part[byte..].find('\n').map(|n| n + byte) {
                if offset + units(&part[..newline]) - start >= max / 4 {
                    break;
                }
                let mut kept = vec![part[newline + 1..].to_owned()];
                kept.extend(parts[index + 1..].iter().cloned());
                return (kept, true);
            }
        }
        offset += len;
    }
    (vec![tail(&parts.concat(), max).0], true)
}

impl TerminalOutput {
    /// `appendTerminalOutput`.
    pub(crate) fn append(&self, chunk: &str) -> Self {
        self.append_bounded(chunk, TAIL_CHARS)
    }

    fn append_bounded(&self, chunk: &str, max: usize) -> Self {
        let input = format!("{}{chunk}", self.pending);
        if input.is_empty() {
            return self.clone();
        }
        let screen = &self.screen;
        let mut terminal = Terminal {
            settled_head: screen.settled.0.clone(),
            settled_tail: screen.settled.1.clone(),
            lines: screen.lines.clone(),
            row: screen.row,
            col: screen.col,
            pen: screen.pen.clone(),
            saved: screen.saved,
            dropped: false,
            max_chars: max,
        };
        let pending = terminal.feed(&input);
        let mut truncated = self.truncated;
        let mut window: usize = terminal.lines.iter().map(line_units).sum();
        while window > max && terminal.row > 0 {
            window -= terminal.scroll_out();
        }
        // Lines below a cursor that moved up can't scroll out; they keep only their ends.
        if window > max {
            let mut budget = max as isize;
            for line in &terminal.lines[..=terminal.row] {
                budget -= line_units(line) as isize;
            }
            for row in terminal.row + 1..terminal.lines.len() {
                let length = line_units(&terminal.lines[row]);
                let keep = budget.clamp(0, length as isize) as usize;
                if keep < length {
                    truncated = true;
                    let removed = trim_line_start(&mut terminal.lines[row], length - keep);
                    if let Some((r, c)) = terminal.saved
                        && r == row
                    {
                        terminal.saved = Some((r, c.saturating_sub(removed)));
                    }
                }
                budget -= line_units(&terminal.lines[row]) as isize;
            }
        }
        truncated |= terminal.dropped;
        let (settled, settled_cut) = tail_parts(
            vec![terminal.settled_head.clone(), terminal.settled_tail.clone()],
            max,
        );
        let window_text = terminal
            .lines
            .iter()
            .map(serialize_line)
            .collect::<Vec<_>>()
            .join("\n");
        let mut all = settled.clone();
        all.push(window_text);
        let (parts, tail_cut) = tail_parts(all, max);
        let mut settled = settled.into_iter();
        Self {
            text: parts.concat(),
            truncated: truncated || settled_cut || tail_cut,
            pending,
            screen: Screen {
                settled: (
                    settled.next().unwrap_or_default(),
                    settled.next().unwrap_or_default(),
                ),
                lines: terminal.lines,
                row: terminal.row,
                col: terminal.col,
                pen: terminal.pen,
                saved: terminal.saved,
            },
        }
    }
}

/// `terminalOutputSpans`' style per pen: a colour the palette resolves, inverse swapped, and a
/// readable text colour over a background the palette did not plan for.
pub(crate) struct SpanStyle {
    pub color: Option<u32>,
    pub background: Option<u32>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

fn xterm(index: u32, palette: &[u32; 16]) -> u32 {
    if index < 16 {
        return palette[index as usize];
    }
    if index >= 232 {
        let level = 8 + (index - 232) * 10;
        return level << 16 | level << 8 | level;
    }
    let cube = index - 16;
    let channel = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
    channel(cube / 36) << 16 | channel(cube / 6 % 6) << 8 | channel(cube % 6)
}

fn resolve_color(code: &str, base: u32, palette: &[u32; 16]) -> Option<u32> {
    let parts: Vec<u32> = code.split(';').map(|p| p.parse().unwrap_or(0)).collect();
    let head = parts[0];
    if head == base + 8 {
        return match parts.get(1) {
            Some(5) => Some(xterm(*parts.get(2)?, palette)),
            _ => Some(
                parts.get(2)? << 16
                    | parts.get(3).copied().unwrap_or(0) << 8
                    | parts.get(4).copied().unwrap_or(0),
            ),
        };
    }
    if (base..=base + 7).contains(&head) {
        return Some(palette[(head - base) as usize]);
    }
    if (base + 60..=base + 67).contains(&head) {
        return Some(palette[(head - base - 52) as usize]);
    }
    None
}

fn luminance(rgb: u32) -> f64 {
    let channel = |v: u32| {
        let s = v as f64 / 255.;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgb >> 16 & 255)
        + 0.7152 * channel(rgb >> 8 & 255)
        + 0.0722 * channel(rgb & 255)
}

fn contrast(a: u32, b: u32) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `spanStyle`.
pub(crate) fn span_style(pen: &str, palette: &[u32; 16]) -> Option<SpanStyle> {
    if pen.is_empty() {
        return None;
    }
    let a = parse_pen(pen);
    let mut color = (!a.fg.is_empty())
        .then(|| resolve_color(&a.fg, 30, palette))
        .flatten();
    let mut background = (!a.bg.is_empty())
        .then(|| resolve_color(&a.bg, 40, palette))
        .flatten();
    if a.inverse {
        (color, background) = (background, Some(color.unwrap_or(palette[7])));
    }
    // Badges like "PASS" on green assume a terminal's own palette; keep their text readable.
    if let Some(bg) = background
        && color.is_none_or(|fg| contrast(fg, bg) < 3.)
    {
        color = Some(if luminance(bg) > 0.3 {
            0x1f2328
        } else {
            0xffffff
        });
    }
    Some(SpanStyle {
        color,
        background,
        bold: a.bold,
        dim: a.dim,
        italic: a.italic,
        underline: a.underline,
        strike: a.strike,
    })
}

/// `terminalOutputSpans`: plain text and a style per pen run; past `max` spans the rest is
/// one plain span.
pub(crate) fn spans(text: &str, max: usize) -> (String, Vec<(std::ops::Range<usize>, String)>) {
    let mut plain = String::with_capacity(text.len());
    let mut runs = Vec::new();
    let mut pen = String::new();
    let mut cursor = 0;
    let push = |plain: &mut String, piece: &str, pen: &str, runs: &mut Vec<_>| {
        let start = plain.len();
        plain.push_str(piece);
        if !pen.is_empty() && !piece.is_empty() {
            runs.push((start..plain.len(), pen.to_owned()));
        }
    };
    while cursor < text.len() {
        let Some(escape) = text[cursor..].find("\u{1b}[").map(|e| e + cursor) else {
            break;
        };
        if runs.len() >= max {
            break;
        }
        push(&mut plain, &text[cursor..escape], &pen, &mut runs);
        let Some(end) = text[escape..].find('m').map(|e| e + escape) else {
            break;
        };
        let params = &text[escape + 2..end];
        pen = if params == "0" {
            String::new()
        } else {
            params.strip_prefix("0;").unwrap_or(params).to_owned()
        };
        cursor = end + 1;
    }
    if cursor < text.len() {
        let rest = &text[cursor..];
        if runs.len() >= max {
            let mut stripped = String::new();
            let mut rest = rest;
            while let Some(at) = rest.find("\u{1b}[") {
                stripped.push_str(&rest[..at]);
                match rest[at..].find('m') {
                    Some(m) => rest = &rest[at + m + 1..],
                    None => {
                        rest = "";
                        break;
                    }
                }
            }
            stripped.push_str(rest);
            push(&mut plain, &stripped, "", &mut runs);
        } else {
            push(&mut plain, rest, &pen, &mut runs);
        }
    }
    (plain, runs)
}

#[cfg(test)]
mod tests {
    use super::TerminalOutput;

    fn fold(chunks: &[&str]) -> TerminalOutput {
        chunks
            .iter()
            .fold(TerminalOutput::default(), |state, chunk| {
                state.append(chunk)
            })
    }

    #[test]
    fn carriage_returns_and_erases_rewrite_the_line() {
        assert_eq!(fold(&["10%\r50%\r100%\n"]).text, "100%\n");
        assert_eq!(fold(&["abcdef\r\u{1b}[Kxy"]).text, "xy");
        assert_eq!(fold(&["ab", "\u{8}c"]).text, "ac");
    }

    #[test]
    fn keeps_pens_and_splits_escapes_across_chunks() {
        let state = fold(&["\u{1b}[3", "2mok\u{1b}[0m done"]);
        assert_eq!(state.text, "\u{1b}[0;32mok\u{1b}[0m done");
    }

    #[test]
    fn cursor_up_rewrites_an_earlier_line() {
        let state = fold(&["one\ntwo\n\u{1b}[2Aun"]);
        assert_eq!(state.text, "une\ntwo\n");
    }

    #[test]
    fn settles_old_lines_and_bounds_the_tail() {
        let lines: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let state = TerminalOutput::default().append_bounded(&lines, 200);
        assert!(state.truncated);
        assert!(state.text.ends_with("line 99\n"));
        assert!(super::units(&state.text) <= 200);
    }

    #[test]
    fn spans_resolve_pens() {
        let (plain, runs) = super::spans("a\u{1b}[0;1;31mb\u{1b}[0mc", 2_000);
        assert_eq!(plain, "abc");
        assert_eq!(runs, vec![(1..2, "1;31".to_owned())]);
        let palette = &super::super::work_rows::PALETTE[0];
        let style = super::span_style("7;32", palette).unwrap();
        assert_eq!(style.background, Some(palette[2]));
    }
}
