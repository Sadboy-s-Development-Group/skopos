//! `skopos work` — pick a project under `config.project_root` and hand the
//! terminal over to an agentic CLI (claude, codex, …) inside that directory.
//!
//! Flow: enumerate immediate subdirectories of the project root, drive a
//! crossterm raw-mode selector with the active provider's accent colour,
//! and on Enter `exec` the provider's binary so it inherits this TTY.
//! Skopos itself exits in the process.

use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::Print,
    terminal::{self, Clear, ClearType},
};

use crate::config::Config;
use crate::dim;
use crate::icons::{self, ProjectIcon};
use crate::providers::{self, ProviderId};

/// Hint footer shown under the picker.
const HINT: &str = "  ↑/↓ project  ·  ←/→ provider  ·  enter open  ·  esc cancel";

/// Run the project picker. Returns when the user cancels; on selection
/// `exec`s the provider's CLI and never returns. `provider_override` and
/// `root_override` come from `--provider` / `--root` on the CLI.
pub(crate) fn run(
    config: &Config,
    provider_override: Option<ProviderId>,
    root_override: Option<PathBuf>,
) -> anyhow::Result<()> {
    let root = root_override.unwrap_or_else(|| config.project_root.clone());
    let root = expand_tilde(&root);

    let projects = list_projects(&root)?;
    if projects.is_empty() {
        println!(
            "{}",
            dim(&format!("  no projects found under {}", root.display()))
        );
        return Ok(());
    }

    if !io::stdin().is_terminal() {
        println!("{}", dim("  skopos work needs an interactive terminal"));
        return Ok(());
    }

    let mut picker = Picker {
        root,
        projects,
        cursor: 0,
        scroll: 0,
        provider: provider_override.unwrap_or(config.default_provider),
        previous_rows: 0,
    };

    let outcome = picker.run()?;
    match outcome {
        PickOutcome::Cancelled => Ok(()),
        PickOutcome::Selected { project, provider } => exec_provider(&project, provider),
    }
}

/// A single row in the picker: a project path plus the cached icon we
/// computed for it. We do the icon detection once at listing time so the
/// draw loop is just lookups.
struct Project {
    path: PathBuf,
    icon: ProjectIcon,
}

/// Sorted list of immediate, non-hidden subdirectories of `root`, each
/// with its detected project icon.
fn list_projects(root: &Path) -> anyhow::Result<Vec<Project>> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) => {
            return Err(anyhow::anyhow!(
                "failed to read project root {}: {err}",
                root.display()
            ))
        }
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        if name.starts_with('.') {
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        let icon = icons::detect(&path);
        out.push(Project { path, icon });
    }
    out.sort_by(|a, b| {
        a.path
            .file_name()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .cmp(&b.path.file_name().unwrap_or_default().to_ascii_lowercase())
    });
    Ok(out)
}

enum PickOutcome {
    Cancelled,
    Selected {
        project: PathBuf,
        provider: ProviderId,
    },
}

struct Picker {
    root: PathBuf,
    projects: Vec<Project>,
    cursor: usize,
    scroll: usize,
    provider: ProviderId,
    /// Number of rows we drew last time, so we can wipe them on redraw.
    previous_rows: u16,
}

impl Picker {
    fn run(&mut self) -> anyhow::Result<PickOutcome> {
        terminal::enable_raw_mode()?;
        let result = self.event_loop();
        let _ = self.erase();
        let _ = terminal::disable_raw_mode();
        result
    }

    fn event_loop(&mut self) -> anyhow::Result<PickOutcome> {
        self.draw()?;
        loop {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL)
                        | (KeyCode::Esc, _)
                        | (KeyCode::Char('q'), _) => return Ok(PickOutcome::Cancelled),
                        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                            self.move_cursor(-1);
                        }
                        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                            self.move_cursor(1);
                        }
                        (KeyCode::Home, _) | (KeyCode::PageUp, _) => {
                            self.cursor = 0;
                        }
                        (KeyCode::End, _) | (KeyCode::PageDown, _) => {
                            self.cursor = self.projects.len().saturating_sub(1);
                        }
                        (KeyCode::Right, _) | (KeyCode::Char('l'), _) | (KeyCode::Tab, _) => {
                            self.cycle_provider(1);
                        }
                        (KeyCode::Left, _) | (KeyCode::Char('h'), _) | (KeyCode::BackTab, _) => {
                            self.cycle_provider(-1);
                        }
                        (KeyCode::Enter, _) => {
                            return Ok(PickOutcome::Selected {
                                project: self.projects[self.cursor].path.clone(),
                                provider: self.provider,
                            });
                        }
                        _ => {}
                    }
                    self.draw()?;
                }
                Event::Resize(_, _) => {
                    self.draw()?;
                }
                _ => {}
            }
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        let len = self.projects.len() as isize;
        if len == 0 {
            return;
        }
        let next = (self.cursor as isize + delta).rem_euclid(len);
        self.cursor = next as usize;
    }

    fn cycle_provider(&mut self, delta: isize) {
        let cycle = providers::picker_cycle();
        let current = cycle.iter().position(|p| *p == self.provider).unwrap_or(0) as isize;
        let len = cycle.len() as isize;
        let next = (current + delta).rem_euclid(len);
        self.provider = cycle[next as usize];
    }

    fn draw(&mut self) -> io::Result<()> {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        // Reserve space for header (3 rows) + footer (2 rows).
        let viewport = rows.saturating_sub(5).max(3) as usize;
        let viewport = viewport.min(self.projects.len()).max(1);

        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + viewport {
            self.scroll = self.cursor + 1 - viewport;
        }

        let accent = self.provider.color();
        let chip = rgb_text(
            &format!(" {} ", self.provider.label()),
            0,
            0,
            0,
            Some(accent),
        );
        let header_line = format!("{chip} {}", dim(&self.root.display().to_string()));

        let mut lines: Vec<String> = Vec::new();
        lines.push(header_line);
        lines.push(String::new());

        // Row layout, where ▶/space + icon + name share fixed leading
        // columns so active/inactive rows align vertically.
        //   active  : "  ▶  ICON  NAME …pad…"  ← background = accent
        //   inactive: "     ICON  NAME"        ← icon coloured, name dim
        // Visible prefix budget before the name: 2 (indent) + 1 (caret) +
        //   2 (spaces) + 1 (icon) + 2 (spaces) = 8 cells.
        let name_budget = (cols as usize).saturating_sub(8).max(8);

        for idx in self.scroll..(self.scroll + viewport) {
            if idx >= self.projects.len() {
                lines.push(String::new());
                continue;
            }
            let project = &self.projects[idx];
            let name = project
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("?");
            let truncated = truncate(name, name_budget);
            let icon = project.icon;

            if idx == self.cursor {
                lines.push(active_row(&truncated, icon, accent));
            } else {
                let icon_glyph =
                    rgb_text(icon.glyph, icon.color.0, icon.color.1, icon.color.2, None);
                lines.push(format!("     {icon_glyph}  {}", dim(&truncated)));
            }
        }

        lines.push(String::new());
        lines.push(dim(&truncate(HINT, cols as usize)));

        self.render_lines(&lines)
    }

    /// Replace the picker's previous frame with `lines`, leaving the
    /// cursor parked at the top so the next draw lands in the same spot.
    fn render_lines(&mut self, lines: &[String]) -> io::Result<()> {
        let mut out = io::stdout();
        if self.previous_rows > 0 {
            queue!(
                out,
                cursor::MoveToColumn(0),
                cursor::MoveUp(self.previous_rows - 1)
            )?;
        } else {
            queue!(out, cursor::MoveToColumn(0))?;
        }
        for (idx, line) in lines.iter().enumerate() {
            queue!(out, Clear(ClearType::CurrentLine), Print(line))?;
            if idx + 1 < lines.len() {
                queue!(out, Print("\r\n"))?;
            }
        }
        self.previous_rows = lines.len() as u16;
        out.flush()
    }

    fn erase(&mut self) -> io::Result<()> {
        let mut out = io::stdout();
        if self.previous_rows > 0 {
            queue!(
                out,
                cursor::MoveToColumn(0),
                cursor::MoveUp(self.previous_rows - 1),
                Clear(ClearType::FromCursorDown),
            )?;
        }
        out.flush()
    }
}

/// Replace the current process with `<provider> --` started inside the
/// selected project directory. The terminal is wiped first so the
/// agentic CLI renders from the top of the screen, not under skopos'
/// scrollback.
fn exec_provider(project: &Path, provider: ProviderId) -> anyhow::Result<()> {
    let _ = execute!(
        io::stdout(),
        Clear(ClearType::All),
        Clear(ClearType::Purge),
        cursor::MoveTo(0, 0),
    );

    let command: OsString = provider.command().into();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&command)
            .current_dir(project)
            .exec();
        Err(anyhow::anyhow!(
            "failed to exec `{}`: {err}",
            provider.command()
        ))
    }

    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(&command)
            .current_dir(project)
            .status()
            .map_err(|err| anyhow::anyhow!("failed to spawn `{}`: {err}", provider.command()))?;
        std::process::exit(status.code().unwrap_or(0));
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = s.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    if s == "~" {
        return home_dir();
    }
    path.to_path_buf()
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Truecolor foreground (and optional background) text. Centralised here
/// so the picker can paint provider-accent backgrounds for the chip.
fn rgb_text(text: &str, r: u8, g: u8, b: u8, bg: Option<(u8, u8, u8)>) -> String {
    match bg {
        Some((br, bg_g, bb)) => {
            format!("\x1b[1m\x1b[38;2;{r};{g};{b}m\x1b[48;2;{br};{bg_g};{bb}m{text}\x1b[0m")
        }
        None => format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m"),
    }
}

/// Render the active row as a non-filled highlight: caret + bold,
/// underlined name in the provider's accent colour. The icon keeps its
/// type colour so the project's language is still legible at a glance.
fn active_row(name: &str, icon: ProjectIcon, accent: (u8, u8, u8)) -> String {
    format!(
        "  \x1b[1m\x1b[38;2;{};{};{}m▶\x1b[0m  \x1b[38;2;{};{};{}m{}\x1b[0m  \x1b[1m\x1b[4m\x1b[38;2;{};{};{}m{name}\x1b[0m",
        accent.0,
        accent.1,
        accent.2,
        icon.color.0,
        icon.color.1,
        icon.color.2,
        icon.glyph,
        accent.0,
        accent.1,
        accent.2,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_projects_skips_hidden_and_files() {
        let dir = std::env::temp_dir().join(format!("skopos-work-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(dir.join("alpha")).unwrap();
        fs::create_dir_all(dir.join("beta")).unwrap();
        fs::create_dir_all(dir.join(".hidden")).unwrap();
        fs::write(dir.join("file.txt"), "x").unwrap();

        let projects = list_projects(&dir).unwrap();
        let names: Vec<String> = projects
            .iter()
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn expand_tilde_resolves_home() {
        std::env::set_var("HOME", "/tmp/home");
        let expanded = expand_tilde(Path::new("~/Coding"));
        assert_eq!(expanded, PathBuf::from("/tmp/home/Coding"));
        let plain = expand_tilde(Path::new("/abs"));
        assert_eq!(plain, PathBuf::from("/abs"));
    }
}
