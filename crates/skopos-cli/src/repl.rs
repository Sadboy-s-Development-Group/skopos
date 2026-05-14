//! Interactive Skopos shell: prints the splash, then drives a live-redrawn
//! bordered input box (crossterm raw mode) where the user types `claude`
//! commands. Results print above the box; a fresh box is drawn below.

use std::io::{self, IsTerminal, Write};
use std::path::Path;

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::Print,
    terminal::{self, Clear, ClearType},
};

use crate::{dim, purple, purple_bold, usage_by_model_report, usage_period_report, UsagePeriod};

/// Run the interactive Skopos shell against `db_path`.
///
/// When stdin is not a terminal (piped/redirected) we cannot enter raw mode,
/// so we just print the splash once and return.
pub(crate) async fn run(db_path: &Path) -> anyhow::Result<()> {
    print_splash(db_path).await;

    if !io::stdin().is_terminal() {
        return Ok(());
    }

    let mut input = InputBox::new();
    loop {
        let line = match input.read_line()? {
            Some(line) => line,
            None => {
                println!();
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        println!("{}", dim(&format!("  › {trimmed}")));
        match parse_command(trimmed) {
            Command::Exit => break,
            Command::Help => print!("{}", help_text()),
            Command::Clear => {
                execute!(io::stdout(), Clear(ClearType::All), cursor::MoveTo(0, 0))?;
                print_splash(db_path).await;
                continue;
            }
            Command::Models => match usage_by_model_report(db_path).await {
                Ok(report) => print!("{report}"),
                Err(error) => println!("{}", dim(&format!("  error: {error}"))),
            },
            Command::Period(period) => match usage_period_report(db_path, period).await {
                Ok(report) => print!("{report}"),
                Err(error) => println!("{}", dim(&format!("  error: {error}"))),
            },
            Command::Unknown(raw) => {
                println!(
                    "{}",
                    dim(&format!("  unknown command: {raw} — type 'help'"))
                );
            }
        }
        println!();
    }

    Ok(())
}

async fn print_splash(db_path: &Path) {
    print!("{}", crate::welcome_screen(db_path).await);
    println!();
}

// ===========================================================================
// Command parsing
// ===========================================================================

enum Command {
    Exit,
    Help,
    Clear,
    Models,
    Period(UsagePeriod),
    Unknown(String),
}

fn parse_command(input: &str) -> Command {
    let parts: Vec<&str> = input.split_whitespace().collect();
    match parts.as_slice() {
        ["exit"] | ["quit"] | ["q"] => Command::Exit,
        ["help"] | ["h"] | ["?"] => Command::Help,
        ["clear"] | ["cls"] => Command::Clear,
        ["claude", rest @ ..] => parse_claude(rest),
        _ => Command::Unknown(input.to_string()),
    }
}

fn parse_claude(args: &[&str]) -> Command {
    match args {
        ["-t"] | ["--today"] | ["today"] => Command::Period(UsagePeriod::Today),
        ["-w"] | ["--week"] | ["week"] => Command::Period(UsagePeriod::Week),
        ["-m"] | ["--month"] | ["month"] => Command::Period(UsagePeriod::Month),
        ["models"] | ["-M"] | ["--models"] => Command::Models,
        _ => Command::Unknown(format!("claude {}", args.join(" "))),
    }
}

fn help_text() -> String {
    let mut out = String::new();
    out.push_str(&purple_bold("Commands"));
    out.push('\n');
    for (cmd, desc) in [
        ("claude -t", "usage today"),
        ("claude -w", "usage this week"),
        ("claude -m", "usage this month"),
        ("claude models", "usage grouped by model"),
        ("clear", "clear the screen and redraw the splash"),
        ("help", "show this help"),
        ("exit / quit", "leave skopos"),
    ] {
        out.push_str(&format!(
            "  {}{}\n",
            purple(&format!("{cmd:<16}")),
            dim(desc)
        ));
    }
    out
}

// ===========================================================================
// Bordered input box
// ===========================================================================

/// Prefix columns before the editable text: `│ › ` is 4 cells wide.
const INPUT_COL: usize = 4;
/// Box overhead around the editable text: borders + `│ › ` + ` │`.
const BOX_OVERHEAD: usize = 6;
const MIN_BOX_WIDTH: usize = 24;
const MAX_BOX_WIDTH: usize = 76;
const HINT: &str = "  -t today  ·  -w week  ·  -m month  ·  models";

/// A live-redrawn, single-line input box with history and horizontal scroll.
///
/// The box owns four terminal rows (top border, input, bottom border, hint).
/// Every `draw` leaves the cursor on the input row; every redraw starts by
/// stepping back up to the top border, so the box stays anchored in place.
struct InputBox {
    width: usize,
    buf: Vec<char>,
    cursor: usize,
    scroll: usize,
    history: Vec<String>,
    history_idx: Option<usize>,
}

impl InputBox {
    fn new() -> Self {
        Self {
            width: MAX_BOX_WIDTH,
            buf: Vec::new(),
            cursor: 0,
            scroll: 0,
            history: Vec::new(),
            history_idx: None,
        }
    }

    /// Width of the editable text area inside the box.
    fn text_area(&self) -> usize {
        self.width.saturating_sub(BOX_OVERHEAD)
    }

    fn refresh_width(&mut self) {
        let cols = terminal::size().map(|(c, _)| c as usize).unwrap_or(80);
        self.width = cols.clamp(MIN_BOX_WIDTH, MAX_BOX_WIDTH);
    }

    /// Read one line. Returns `Ok(None)` on Ctrl+C, or Ctrl+D on an empty line.
    /// Raw mode is always restored, even on error.
    fn read_line(&mut self) -> io::Result<Option<String>> {
        terminal::enable_raw_mode()?;
        let result = self.read_line_inner();
        let _ = self.erase();
        let _ = terminal::disable_raw_mode();
        let result = result?;
        if let Some(line) = &result {
            if !line.trim().is_empty() {
                self.history.push(line.clone());
            }
        }
        Ok(result)
    }

    fn read_line_inner(&mut self) -> io::Result<Option<String>> {
        self.buf.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.history_idx = None;
        self.refresh_width();
        self.draw(true)?;

        loop {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(None),
                        (KeyCode::Char('d'), KeyModifiers::CONTROL) if self.buf.is_empty() => {
                            return Ok(None)
                        }
                        (KeyCode::Enter, _) => return Ok(Some(self.buf.iter().collect())),
                        (KeyCode::Backspace, _) if self.cursor > 0 => {
                            self.cursor -= 1;
                            self.buf.remove(self.cursor);
                        }
                        (KeyCode::Delete, _) if self.cursor < self.buf.len() => {
                            self.buf.remove(self.cursor);
                        }
                        (KeyCode::Left, _) => self.cursor = self.cursor.saturating_sub(1),
                        (KeyCode::Right, _) if self.cursor < self.buf.len() => {
                            self.cursor += 1;
                        }
                        (KeyCode::Home, _) => self.cursor = 0,
                        (KeyCode::End, _) => self.cursor = self.buf.len(),
                        (KeyCode::Up, _) => self.history_prev(),
                        (KeyCode::Down, _) => self.history_next(),
                        (KeyCode::Char(c), m)
                            if !m.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                        {
                            self.buf.insert(self.cursor, c);
                            self.cursor += 1;
                        }
                        _ => {}
                    }
                    self.draw(false)?;
                }
                Event::Resize(_, _) => {
                    self.refresh_width();
                    self.draw(false)?;
                }
                _ => {}
            }
        }
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_idx {
            None => self.history.len() - 1,
            Some(i) => i.saturating_sub(1),
        };
        self.history_idx = Some(idx);
        let entry = self.history[idx].clone();
        self.set_buf(&entry);
    }

    fn history_next(&mut self) {
        match self.history_idx {
            Some(i) if i + 1 < self.history.len() => {
                self.history_idx = Some(i + 1);
                let entry = self.history[i + 1].clone();
                self.set_buf(&entry);
            }
            Some(_) => {
                self.history_idx = None;
                self.set_buf("");
            }
            None => {}
        }
    }

    fn set_buf(&mut self, text: &str) {
        self.buf = text.chars().collect();
        self.cursor = self.buf.len();
        self.scroll = 0;
    }

    /// Draw (or redraw) the four box rows. `first` skips the step back up to
    /// the top border that an in-place redraw needs.
    fn draw(&mut self, first: bool) -> io::Result<()> {
        let area = self.text_area();
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor > self.scroll + area {
            self.scroll = self.cursor - area;
        }

        let inner = self.width - 2;
        let top = format!("╭{}╮", "─".repeat(inner));
        let bottom = format!("╰{}╯", "─".repeat(inner));
        let visible: String = self.buf.iter().skip(self.scroll).take(area).collect();
        let visible_len = self.buf.len().saturating_sub(self.scroll).min(area);
        let pad = " ".repeat(area - visible_len);
        let bar = purple("│");
        let input_line = format!("{bar} {} {visible}{pad} {bar}", purple("›"));

        let mut out = io::stdout();
        if !first {
            queue!(out, cursor::MoveToColumn(0), cursor::MoveUp(1))?;
        }
        queue!(
            out,
            Clear(ClearType::CurrentLine),
            Print(purple(&top)),
            Print("\r\n"),
            Clear(ClearType::CurrentLine),
            Print(&input_line),
            Print("\r\n"),
            Clear(ClearType::CurrentLine),
            Print(purple(&bottom)),
            Print("\r\n"),
            Clear(ClearType::CurrentLine),
            Print(dim(HINT)),
        )?;
        let col = (INPUT_COL + self.cursor - self.scroll) as u16;
        queue!(out, cursor::MoveUp(2), cursor::MoveToColumn(col))?;
        out.flush()
    }

    /// Wipe the four box rows, leaving the cursor where the box began so the
    /// caller's output takes its place.
    fn erase(&mut self) -> io::Result<()> {
        let mut out = io::stdout();
        queue!(out, cursor::MoveToColumn(0), cursor::MoveUp(1))?;
        for row in 0..4 {
            queue!(out, Clear(ClearType::CurrentLine))?;
            if row < 3 {
                queue!(out, cursor::MoveDown(1))?;
            }
        }
        queue!(out, cursor::MoveUp(3), cursor::MoveToColumn(0))?;
        out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_period_aliases() {
        assert!(matches!(
            parse_command("claude -t"),
            Command::Period(UsagePeriod::Today)
        ));
        assert!(matches!(
            parse_command("claude -w"),
            Command::Period(UsagePeriod::Week)
        ));
        assert!(matches!(
            parse_command("  claude   -m  "),
            Command::Period(UsagePeriod::Month)
        ));
        assert!(matches!(parse_command("claude models"), Command::Models));
    }

    #[test]
    fn parses_control_commands() {
        assert!(matches!(parse_command("exit"), Command::Exit));
        assert!(matches!(parse_command("quit"), Command::Exit));
        assert!(matches!(parse_command("help"), Command::Help));
        assert!(matches!(parse_command("clear"), Command::Clear));
    }

    #[test]
    fn unknown_command_is_reported() {
        assert!(matches!(parse_command("claude xyz"), Command::Unknown(_)));
        assert!(matches!(parse_command("frobnicate"), Command::Unknown(_)));
    }
}
