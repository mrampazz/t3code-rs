//! Terminal emulation for the thread terminal drawer (web: `ThreadTerminalDrawer.tsx`).
//! The PTY lives on the server; this keeps a VT grid fed by `terminal.attach` output.

use std::ops::Range;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};

pub const TERMINAL_ID: &str = "term-1";
/// `DEFAULT_MAX_TERMINAL_BUFFER_BYTES` (packages/contracts/src/terminal.ts).
pub const MAX_HISTORY_BYTES: usize = 512 * 1024;

/// Collects the emulator's answers to terminal queries (DA, DSR) for `terminal.write`.
#[derive(Clone, Default)]
struct Replies(Arc<Mutex<Vec<String>>>);

impl EventListener for Replies {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            self.0.lock().unwrap().push(text);
        }
    }
}

pub struct TermView {
    term: Term<Replies>,
    parser: Processor,
    replies: Replies,
    pub cols: usize,
    pub rows: usize,
}

/// One styled run in a rendered line (byte range into `Line::text`).
#[derive(Debug, PartialEq)]
pub struct Run {
    pub range: Range<usize>,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
}

#[derive(Debug, Default)]
pub struct Line {
    pub text: String,
    pub runs: Vec<Run>,
}

const FOREGROUND: Rgb = Rgb {
    r: 0xd4,
    g: 0xd4,
    b: 0xd4,
};
const BACKGROUND: Rgb = Rgb {
    r: 0x1e,
    g: 0x1e,
    b: 0x1e,
};

impl TermView {
    pub fn new(cols: usize, rows: usize) -> Self {
        let replies = Replies::default();
        let term = Term::new(
            Config::default(),
            &TermSize::new(cols, rows),
            replies.clone(),
        );
        Self {
            term,
            parser: Processor::new(),
            replies,
            cols,
            rows,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.cols, self.rows);
    }

    pub fn feed(&mut self, data: &str) {
        self.parser.advance(&mut self.term, data.as_bytes());
    }

    pub fn take_replies(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.replies.0.lock().unwrap())
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        (self.cols, self.rows) = (cols, rows);
        self.term.resize(TermSize::new(cols, rows));
    }

    /// The visible screen as styled lines, with the cursor drawn as an inverse cell.
    pub fn lines(&self) -> Vec<Line> {
        let content = self.term.renderable_content();
        let cursor = content.cursor.point;
        let mut lines: Vec<Line> = (0..self.rows).map(|_| Line::default()).collect();
        for indexed in content.display_iter {
            let row = (indexed.point.line.0 + content.display_offset as i32) as usize;
            let Some(line) = lines.get_mut(row) else {
                continue;
            };
            let cell = indexed.cell;
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let mut fg = resolve(cell.fg, true);
            let mut bg = match cell.bg {
                Color::Named(NamedColor::Background) => None,
                other => Some(resolve(other, false)),
            };
            if cell.flags.contains(Flags::INVERSE) || indexed.point == cursor {
                let swapped = bg.unwrap_or(BACKGROUND);
                bg = Some(fg);
                fg = swapped;
            }
            let bold = cell.flags.contains(Flags::BOLD);
            let start = line.text.len();
            line.text.push(if cell.c == '\0' { ' ' } else { cell.c });
            let end = line.text.len();
            match line.runs.last_mut() {
                Some(run)
                    if run.fg == fg
                        && run.bg == bg
                        && run.bold == bold
                        && run.range.end == start =>
                {
                    run.range.end = end
                }
                _ => line.runs.push(Run {
                    range: start..end,
                    fg,
                    bg,
                    bold,
                }),
            }
        }
        for line in &mut lines {
            let trimmed = line.text.trim_end().len();
            // Keep trailing cells that carry a background (the cursor, highlighted blanks).
            let keep = line
                .runs
                .iter()
                .filter(|r| r.bg.is_some())
                .map(|r| r.range.end)
                .max()
                .unwrap_or(0);
            let cut = trimmed.max(keep);
            line.text.truncate(cut);
            line.runs.retain(|r| r.range.start < cut);
            if let Some(last) = line.runs.last_mut() {
                last.range.end = last.range.end.min(cut);
            }
        }
        lines
    }
}

/// xterm palette: 16 ANSI colors, the 6×6×6 cube, and the grayscale ramp.
fn resolve(color: Color, foreground: bool) -> Rgb {
    const ANSI: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x31, 0x31),
        (0x0d, 0xbc, 0x79),
        (0xe5, 0xe5, 0x10),
        (0x24, 0x72, 0xc8),
        (0xbc, 0x3f, 0xbc),
        (0x11, 0xa8, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x66, 0x66, 0x66),
        (0xf1, 0x4c, 0x4c),
        (0x23, 0xd1, 0x8b),
        (0xf5, 0xf5, 0x43),
        (0x3b, 0x8e, 0xea),
        (0xd6, 0x70, 0xd6),
        (0x29, 0xb8, 0xdb),
        (0xff, 0xff, 0xff),
    ];
    let indexed = |i: u8| -> Rgb {
        match i {
            0..16 => {
                let (r, g, b) = ANSI[i as usize];
                Rgb { r, g, b }
            }
            16..232 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgb {
                    r: level(i / 36),
                    g: level(i / 6 % 6),
                    b: level(i % 6),
                }
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                Rgb { r: v, g: v, b: v }
            }
        }
    };
    match color {
        Color::Spec(rgb) => rgb,
        Color::Indexed(i) => indexed(i),
        Color::Named(named) => match named as usize {
            n @ 0..16 => indexed(n as u8),
            n if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&n) => {
                indexed((n - NamedColor::DimBlack as usize) as u8)
            }
            _ if named == NamedColor::Background => BACKGROUND,
            _ if foreground => FOREGROUND,
            _ => BACKGROUND,
        },
    }
}

/// Bytes a key press sends to the PTY (xterm conventions). `None` for keys the app handles.
pub fn key_bytes(
    key: &str,
    key_char: Option<&str>,
    control: bool,
    alt: bool,
    platform: bool,
) -> Option<String> {
    if platform {
        return None;
    }
    let named = match key {
        "enter" => Some("\r"),
        "backspace" => Some("\x7f"),
        "tab" => Some("\t"),
        "escape" => Some("\x1b"),
        "up" => Some("\x1b[A"),
        "down" => Some("\x1b[B"),
        "right" => Some("\x1b[C"),
        "left" => Some("\x1b[D"),
        "home" => Some("\x1b[H"),
        "end" => Some("\x1b[F"),
        "delete" => Some("\x1b[3~"),
        "pageup" => Some("\x1b[5~"),
        "pagedown" => Some("\x1b[6~"),
        _ => None,
    };
    if let Some(seq) = named {
        return Some(if alt {
            format!("\x1b{seq}")
        } else {
            seq.to_string()
        });
    }
    if control {
        let c = key
            .chars()
            .next()
            .filter(|_| key.chars().count() == 1)?
            .to_ascii_lowercase();
        let code = match c {
            'a'..='z' => c as u8 - b'a' + 1,
            '[' => 0x1b,
            '\\' => 0x1c,
            ']' => 0x1d,
            ' ' | '@' => 0,
            _ => return None,
        };
        return Some((code as char).to_string());
    }
    let text = key_char
        .map(str::to_owned)
        .or_else(|| (key == "space").then(|| " ".into()))?;
    Some(if alt { format!("\x1b{text}") } else { text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_text_colors_and_cursor() {
        let mut view = TermView::new(20, 3);
        view.feed("hi \x1b[31mred\x1b[0m\r\nnext");
        let lines = view.lines();
        assert_eq!(lines[0].text, "hi red");
        let red = lines[0]
            .runs
            .iter()
            .find(|r| &lines[0].text[r.range.clone()] == "red")
            .unwrap();
        assert_eq!(
            red.fg,
            Rgb {
                r: 0xcd,
                g: 0x31,
                b: 0x31
            }
        );
        // The cursor sits after "next" as an inverse blank.
        assert_eq!(lines[1].text, "next ");
        assert_eq!(lines[1].runs.last().unwrap().bg, Some(FOREGROUND));
        assert_eq!(lines[2].text, "");
    }

    #[test]
    fn clear_screen_and_replies() {
        let mut view = TermView::new(10, 2);
        view.feed("junk\x1b[2J\x1b[Hok");
        assert_eq!(view.lines()[0].text.trim_end(), "ok");
        view.feed("\x1b[c"); // primary device attributes query
        assert!(!view.take_replies().is_empty());
    }

    #[test]
    fn maps_keys_to_pty_bytes() {
        assert_eq!(
            key_bytes("enter", None, false, false, false).as_deref(),
            Some("\r")
        );
        assert_eq!(
            key_bytes("c", Some("c"), true, false, false).as_deref(),
            Some("\x03")
        );
        assert_eq!(
            key_bytes("a", Some("A"), false, false, false).as_deref(),
            Some("A")
        );
        assert_eq!(
            key_bytes("left", None, false, true, false).as_deref(),
            Some("\x1b\x1b[D")
        );
        assert_eq!(
            key_bytes("v", Some("v"), false, false, true),
            None,
            "cmd shortcuts stay with the app"
        );
        assert_eq!(
            key_bytes("space", None, false, false, false).as_deref(),
            Some(" ")
        );
    }
}
