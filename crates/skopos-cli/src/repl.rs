//! Interactive Skopos shell: prints the splash, then drives a live-redrawn
//! bordered input box (crossterm raw mode) where the user types commands.
//! Results print above the box; a fresh box is drawn below.
//!
//! The box spans the full terminal width and re-fits on resize. While the
//! splash is still the only thing on screen, a resize also reflows it.

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::Print,
    terminal::{self, Clear, ClearType},
};

use crate::{
    config, dim, providers_report, purple, purple_bold, usage_by_model_report_filtered,
    usage_limits_report, usage_period_report_filtered, work, UsagePeriod,
};

/// Run the interactive Skopos shell against `db_path`.
///
/// When stdin is not a terminal (piped/redirected) we cannot enter raw mode,
/// so we just print the splash once and return.
pub(crate) async fn run(db_path: &Path) -> anyhow::Result<()> {
    print_splash();

    if !io::stdin().is_terminal() {
        return Ok(());
    }

    let mut input = InputBox::new();
    // `fresh_screen`: the splash is still the only thing on screen, so a
    // resize can clear and reflow it. `fresh_line`: start the next read with
    // an empty buffer — `false` resumes the buffer after a resize.
    let mut fresh_screen = true;
    let mut fresh_line = true;

    loop {
        match input.read_line(fresh_line)? {
            ReadOutcome::Eof => {
                println!();
                break;
            }
            ReadOutcome::Resized => {
                if fresh_screen {
                    execute!(io::stdout(), Clear(ClearType::All), cursor::MoveTo(0, 0))?;
                    print_splash();
                }
                fresh_line = false;
                continue;
            }
            ReadOutcome::Line(line) => {
                fresh_line = true;
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
                        print_splash();
                        fresh_screen = true;
                        continue;
                    }
                    Command::Providers => report_or_error(providers_report(db_path).await),
                    Command::Models(provider) => report_or_error(
                        usage_by_model_report_filtered(db_path, provider.as_deref()).await,
                    ),
                    Command::Period(period, provider) => report_or_error(
                        usage_period_report_filtered(db_path, period, provider.as_deref()).await,
                    ),
                    Command::Usage => report_or_error(usage_limits_report().await),
                    Command::UsageInstallHint => {
                        println!(
                            "{}",
                            dim(
                                "  run from the shell: `skopos usage install` (modifies ~/.claude/settings.json)"
                            )
                        );
                    }
                    Command::Work => match config::load() {
                        Ok(cfg) => {
                            if let Err(error) = work::run(&cfg, None, None) {
                                println!("{}", dim(&format!("  error: {error}")));
                            }
                        }
                        Err(error) => {
                            println!("{}", dim(&format!("  config error: {error}")));
                        }
                    },
                    Command::Unknown(raw) => {
                        println!(
                            "{}",
                            dim(&format!("  unknown command: {raw} — type 'help'"))
                        );
                    }
                }
                fresh_screen = false;
                println!();
            }
        }
    }

    Ok(())
}

fn report_or_error(result: anyhow::Result<String>) {
    match result {
        Ok(report) => print!("{report}"),
        Err(error) => println!("{}", dim(&format!("  error: {error}"))),
    }
}

fn print_splash() {
    let width = terminal::size().map(|(c, _)| c as usize).unwrap_or(80);
    print!("{}", crate::welcome_screen(width));
    println!();
}

// ===========================================================================
// Command parsing
// ===========================================================================

enum Command {
    Exit,
    Help,
    Clear,
    Providers,
    Models(Option<String>),
    Period(UsagePeriod, Option<String>),
    Work,
    Usage,
    UsageInstallHint,
    Unknown(String),
}

fn parse_command(input: &str) -> Command {
    let parts: Vec<&str> = input.split_whitespace().collect();
    match parts.as_slice() {
        ["exit"] | ["quit"] | ["q"] => Command::Exit,
        ["help"] | ["h"] | ["?"] => Command::Help,
        ["clear"] | ["cls"] => Command::Clear,
        ["providers"] | ["p"] => Command::Providers,
        ["work"] | ["w"] => Command::Work,
        ["usage"] | ["u"] => Command::Usage,
        ["usage", "install"] | ["usage", "uninstall"] => Command::UsageInstallHint,
        ["claude", rest @ ..] => parse_period_args("claude", rest, Some("anthropic")),
        ["codex", rest @ ..] => parse_period_args("codex", rest, Some("openai")),
        ["gemini", rest @ ..] => parse_period_args("gemini", rest, Some("google")),
        _ => Command::Unknown(input.to_string()),
    }
}

fn parse_period_args(prefix: &str, args: &[&str], provider: Option<&str>) -> Command {
    let provider = provider.map(ToString::to_string);
    match args {
        ["-t"] | ["--today"] | ["today"] => Command::Period(UsagePeriod::Today, provider),
        ["-w"] | ["--week"] | ["week"] => Command::Period(UsagePeriod::Week, provider),
        ["-m"] | ["--month"] | ["month"] => Command::Period(UsagePeriod::Month, provider),
        ["models"] | ["-M"] | ["--models"] => Command::Models(provider),
        _ => Command::Unknown(format!("{prefix} {}", args.join(" "))),
    }
}

fn help_text() -> String {
    let mut out = String::new();
    out.push_str(&purple_bold("Commands"));
    out.push('\n');
    for (cmd, desc) in [
        ("work", "pick a project and launch the agentic CLI"),
        ("usage", "5h / weekly rate-limit bars per provider"),
        ("usage install", "register statusline hook (run from shell)"),
        ("claude -t", "Claude usage today (token totals)"),
        ("claude -w", "Claude usage this week (token totals)"),
        ("claude -m", "Claude usage this month (token totals)"),
        ("claude models", "Claude usage grouped by model"),
        ("codex -t", "Codex usage today (token totals)"),
        ("codex -w", "Codex usage this week (token totals)"),
        ("codex -m", "Codex usage this month (token totals)"),
        ("codex models", "Codex usage grouped by model"),
        ("gemini -t", "Gemini usage today (token totals)"),
        ("gemini -w", "Gemini usage this week (token totals)"),
        ("gemini -m", "Gemini usage this month (token totals)"),
        ("gemini models", "Gemini usage grouped by model"),
        ("providers", "list tracked providers"),
        (
            "claude import",
            "import Claude Code logs (run: skopos claude import)",
        ),
        (
            "codex import",
            "import Codex rollout JSONLs (run: skopos codex import)",
        ),
        (
            "gemini import",
            "import Gemini session JSONLs (run: skopos gemini import)",
        ),
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
/// Smallest box we will draw; below this a terminal is too narrow to use.
const MIN_BOX_WIDTH: usize = 20;
const HINT: &str =
    "  work  ·  usage  ·  -t today  ·  -w week  ·  -m month  ·  models  ·  providers";

/// What a `read_line` call ended with.
enum ReadOutcome {
    /// The user submitted a line.
    Line(String),
    /// Ctrl+C, or Ctrl+D on an empty buffer.
    Eof,
    /// The terminal was resized; the caller decides what to reflow.
    Resized,
}

/// A live-redrawn, single-line input box with history and horizontal scroll.
///
/// The box owns four terminal rows (top border, input, bottom border, hint)
/// and spans the full terminal width. Every `draw` leaves the cursor on the
/// input row; every redraw steps back up to the top border first, so the box
/// stays anchored in place.
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
            width: 80,
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

    /// Resize the box to the full terminal width.
    fn refresh_width(&mut self) {
        let cols = terminal::size().map(|(c, _)| c as usize).unwrap_or(80);
        self.width = cols.max(MIN_BOX_WIDTH);
    }

    /// Read one line. `fresh` starts with an empty buffer; `false` resumes the
    /// current buffer (used to continue editing after a resize). Raw mode is
    /// always restored, even on error.
    fn read_line(&mut self, fresh: bool) -> io::Result<ReadOutcome> {
        terminal::enable_raw_mode()?;
        if fresh {
            self.buf.clear();
            self.cursor = 0;
            self.scroll = 0;
            self.history_idx = None;
        }
        let result = self.read_line_inner();
        let _ = self.erase();
        let _ = terminal::disable_raw_mode();
        let outcome = result?;
        if let ReadOutcome::Line(line) = &outcome {
            if !line.trim().is_empty() {
                self.history.push(line.clone());
            }
        }
        Ok(outcome)
    }

    fn read_line_inner(&mut self) -> io::Result<ReadOutcome> {
        self.refresh_width();
        self.draw(true)?;

        loop {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(ReadOutcome::Eof),
                        (KeyCode::Char('d'), KeyModifiers::CONTROL) if self.buf.is_empty() => {
                            return Ok(ReadOutcome::Eof)
                        }
                        (KeyCode::Enter, _) => {
                            return Ok(ReadOutcome::Line(self.buf.iter().collect()))
                        }
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
                    // Coalesce a burst of resize events from a window drag.
                    while event::poll(Duration::ZERO)? {
                        if !matches!(event::read()?, Event::Resize(_, _)) {
                            break;
                        }
                    }
                    return Ok(ReadOutcome::Resized);
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
        let hint: String = HINT.chars().take(self.width).collect();

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
            Print(dim(&hint)),
        )?;
        let col = (INPUT_COL + self.cursor - self.scroll) as u16;
        queue!(out, cursor::MoveUp(2), cursor::MoveToColumn(col))?;
        out.flush()
    }

    /// Wipe the box (and anything below it), leaving the cursor where the box
    /// began so the caller's output takes its place. Clearing from the cursor
    /// down also tidies up fragments left by a shrink-resize.
    fn erase(&mut self) -> io::Result<()> {
        let mut out = io::stdout();
        queue!(
            out,
            cursor::MoveToColumn(0),
            cursor::MoveUp(1),
            Clear(ClearType::FromCursorDown),
        )?;
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
            Command::Period(UsagePeriod::Today, Some(ref p)) if p == "anthropic"
        ));
        assert!(matches!(
            parse_command("claude -w"),
            Command::Period(UsagePeriod::Week, Some(ref p)) if p == "anthropic"
        ));
        assert!(matches!(
            parse_command("  claude   -m  "),
            Command::Period(UsagePeriod::Month, Some(ref p)) if p == "anthropic"
        ));
        assert!(matches!(
            parse_command("claude models"),
            Command::Models(Some(ref p)) if p == "anthropic"
        ));
    }

    #[test]
    fn parses_gemini_period_aliases() {
        assert!(matches!(
            parse_command("gemini -t"),
            Command::Period(UsagePeriod::Today, Some(ref p)) if p == "google"
        ));
        assert!(matches!(
            parse_command("gemini -w"),
            Command::Period(UsagePeriod::Week, Some(ref p)) if p == "google"
        ));
        assert!(matches!(
            parse_command("gemini -m"),
            Command::Period(UsagePeriod::Month, Some(ref p)) if p == "google"
        ));
        assert!(matches!(
            parse_command("gemini models"),
            Command::Models(Some(ref p)) if p == "google"
        ));
    }

    #[test]
    fn parses_codex_period_aliases() {
        assert!(matches!(
            parse_command("codex -t"),
            Command::Period(UsagePeriod::Today, Some(ref p)) if p == "openai"
        ));
        assert!(matches!(
            parse_command("codex -w"),
            Command::Period(UsagePeriod::Week, Some(ref p)) if p == "openai"
        ));
        assert!(matches!(
            parse_command("codex -m"),
            Command::Period(UsagePeriod::Month, Some(ref p)) if p == "openai"
        ));
        assert!(matches!(
            parse_command("codex models"),
            Command::Models(Some(ref p)) if p == "openai"
        ));
    }

    #[test]
    fn parses_control_commands() {
        assert!(matches!(parse_command("exit"), Command::Exit));
        assert!(matches!(parse_command("quit"), Command::Exit));
        assert!(matches!(parse_command("help"), Command::Help));
        assert!(matches!(parse_command("clear"), Command::Clear));
        assert!(matches!(parse_command("providers"), Command::Providers));
    }

    #[test]
    fn unknown_command_is_reported() {
        assert!(matches!(parse_command("claude xyz"), Command::Unknown(_)));
        assert!(matches!(parse_command("frobnicate"), Command::Unknown(_)));
    }
}
