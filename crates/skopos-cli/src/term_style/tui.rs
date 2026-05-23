//! Interactive picker for `skopos term-style`.
//!
//! Layout: a picker list on the left, the focused profile's ASCII art
//! and palette swatches on the right. The currently active profile is
//! marked with a coloured "● active · locked in" badge that uses that
//! profile's OS palette colour, so the user can spot it at a glance.

use std::io::{self, Write};
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::Print,
    terminal::{self, Clear, ClearType},
};

use super::apply::{apply, ApplyReport};
use super::profile::{Discovery, Palette, Profile};

const PICKER_WIDTH: usize = 24;
const GAP: usize = 4;
const FOOTER: &str = "  \x1b[38;2;140;140;140m↑/↓\x1b[0m pick · \x1b[38;2;140;140;140m↵\x1b[0m apply · \x1b[38;2;140;140;140mq\x1b[0m quit";

pub(crate) fn run(mut discovery: Discovery) -> Result<()> {
    if discovery.profiles.is_empty() {
        anyhow::bail!(
            "no profiles found under {}/profiles/",
            discovery.fastfetch_root.display()
        );
    }

    enter_screen()?;
    let result = main_loop(&mut discovery);
    let _ = leave_screen();
    let outcome = result?;

    if let Some((name, report)) = outcome {
        println!(
            "Active profile: \x1b[1m{name}\x1b[0m — open a new terminal or run 'fastfetch' to see it."
        );
        if report.omp_skipped {
            println!("  (oh-my-posh theme.omp.json not found; only fastfetch was updated.)");
        }
    }
    Ok(())
}

fn main_loop(d: &mut Discovery) -> Result<Option<(String, ApplyReport)>> {
    let mut cursor = d
        .active
        .as_ref()
        .and_then(|a| d.profiles.iter().position(|p| &p.name == a))
        .unwrap_or(0);

    loop {
        draw(d, cursor)?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
            | (KeyCode::Esc, _)
            | (KeyCode::Char('q'), _) => return Ok(None),
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                if cursor > 0 {
                    cursor -= 1;
                }
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                if cursor + 1 < d.profiles.len() {
                    cursor += 1;
                }
            }
            (KeyCode::Home, _) | (KeyCode::Char('g'), _) => cursor = 0,
            (KeyCode::End, _) | (KeyCode::Char('G'), _) => cursor = d.profiles.len() - 1,
            (KeyCode::Enter, _) => {
                let profile = &d.profiles[cursor];
                let report = apply(&d.fastfetch_root, profile)?;
                d.active = Some(profile.name.clone());
                return Ok(Some((profile.name.clone(), report)));
            }
            _ => {}
        }
    }
}

fn draw(d: &Discovery, cursor_idx: usize) -> io::Result<()> {
    let mut out = io::stdout();
    queue!(out, Clear(ClearType::All), cursor::MoveTo(0, 0))?;

    let profile = &d.profiles[cursor_idx];
    let active = d.active.as_deref();
    let picker = render_picker(&d.profiles, cursor_idx, active);
    let preview = render_preview(profile, active == Some(profile.name.as_str()));

    let height = picker.len().max(preview.len());
    for row in 0..height {
        let left = picker.get(row).map(String::as_str).unwrap_or("");
        let right = preview.get(row).map(String::as_str).unwrap_or("");
        let left_pad = PICKER_WIDTH.saturating_sub(visible_width(left));
        queue!(
            out,
            Print(left),
            Print(" ".repeat(left_pad)),
            Print(" ".repeat(GAP)),
            Print(right),
            Print("\r\n"),
        )?;
    }
    queue!(out, Print("\r\n"), Print(FOOTER), Print("\r\n"))?;
    out.flush()
}

fn render_picker(profiles: &[Profile], cursor_idx: usize, active: Option<&str>) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("\x1b[1m┌─ Profiles ───────────┐\x1b[0m".into());
    for (i, p) in profiles.iter().enumerate() {
        let is_cursor = i == cursor_idx;
        let is_active = active == Some(p.name.as_str());
        let cursor_glyph = if is_cursor { ">" } else { " " };
        let name = if is_cursor {
            format!("\x1b[1m{}\x1b[0m", p.name)
        } else {
            p.name.clone()
        };
        let badge = if is_active {
            format!(" {}", hex_fg(&p.palette.omp_os, "●"))
        } else {
            String::new()
        };
        // The visible content width is PICKER_WIDTH minus the borders + spaces;
        // we let the draw routine pad to the full picker width.
        lines.push(format!("│ {cursor_glyph} {name}{badge}"));
    }
    lines.push("\x1b[1m└──────────────────────┘\x1b[0m".into());
    lines
}

fn render_preview(profile: &Profile, is_active: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let title = if is_active {
        format!(
            "{}  {}",
            hex_fg_bold(&profile.palette.omp_os, &profile.name),
            hex_fg(&profile.palette.omp_session, "● active · locked in"),
        )
    } else {
        hex_fg_bold(&profile.palette.omp_os, &profile.name)
    };
    lines.push(title);
    lines.push(String::new());
    for line in profile.logo.lines() {
        lines.push(line.to_string());
    }
    lines.push(String::new());
    lines.push(render_swatches(&profile.palette));
    lines
}

fn render_swatches(p: &Palette) -> String {
    let entries = [
        ("os", &p.omp_os),
        ("session", &p.omp_session),
        ("path", &p.omp_path),
        ("git", &p.omp_git),
        ("closer", &p.omp_closer),
    ];
    let mut s = String::from("\x1b[38;2;140;140;140mPalette \x1b[0m ");
    for (i, (label, hex)) in entries.iter().enumerate() {
        if i > 0 {
            s.push_str("  ");
        }
        // Solid block in the palette colour, then the hex string dim grey.
        s.push_str(&hex_fg(hex, "███"));
        s.push(' ');
        s.push_str(&format!(
            "\x1b[38;2;140;140;140m{label}\x1b[0m \x1b[1m{hex}\x1b[0m"
        ));
    }
    s
}

/// Wrap text in a truecolor foreground escape derived from a `#RRGGBB`
/// hex string. On parse failure, returns the text uncoloured rather than
/// failing the render — the picker stays usable even if a palette entry
/// is malformed.
fn hex_fg(hex: &str, text: &str) -> String {
    match hex_to_rgb(hex) {
        Some((r, g, b)) => format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m"),
        None => text.to_string(),
    }
}

fn hex_fg_bold(hex: &str, text: &str) -> String {
    match hex_to_rgb(hex) {
        Some((r, g, b)) => format!("\x1b[1m\x1b[38;2;{r};{g};{b}m{text}\x1b[0m"),
        None => format!("\x1b[1m{text}\x1b[0m"),
    }
}

fn hex_to_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Visible (printable) width of an ANSI-coloured string. Treats every
/// `ESC [ ... m` sequence as zero-width and counts everything else as
/// one column — the logos use only ASCII glyphs plus colour codes, so
/// we don't need full grapheme/wcwidth handling.
fn visible_width(s: &str) -> usize {
    let mut n = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j] != b'm' {
                j += 1;
            }
            i = j + 1;
            continue;
        }
        n += 1;
        i += 1;
    }
    n
}

fn enter_screen() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), terminal::EnterAlternateScreen, cursor::Hide)
}

fn leave_screen() -> io::Result<()> {
    execute!(io::stdout(), cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()
}
