//! The Skopos splash screen: curated ASCII art with a purple vertical
//! gradient, plus a branding/command side panel. The layout is responsive
//! to terminal width; the art itself is never scaled or truncated.

use crate::theme::{dim, purple, purple_bold, rgb};

const SKOPOS_ASCII: &str = include_str!("../assets/skopos-ascii.txt");

/// Horizontal gap between the ASCII art and the side panel.
const SPLASH_GAP: usize = 4;

/// Render the splash for the given terminal `width`, picking a responsive
/// layout. The ASCII art is never scaled or truncated (curated asset); the
/// layout adapts instead:
/// - wide: art and panel side by side,
/// - medium: art stacked above the panel,
/// - narrow: panel only.
pub(crate) fn welcome_screen(width: usize) -> String {
    let art_lines: Vec<&str> = SKOPOS_ASCII.trim_end_matches('\n').lines().collect();
    let info_lines = panel_info_lines();
    let art_width = art_lines
        .iter()
        .map(|line| visible_width(line))
        .max()
        .unwrap_or(0);
    let panel_width = info_lines
        .iter()
        .map(InfoLine::visible_len)
        .max()
        .unwrap_or(0);

    if width >= art_width + SPLASH_GAP + panel_width {
        render_splash_side_by_side(&art_lines, &info_lines, art_width)
    } else if width >= art_width {
        render_splash_stacked(&art_lines, &info_lines)
    } else {
        render_splash_compact(&info_lines)
    }
}

fn render_splash_side_by_side(
    art_lines: &[&str],
    info_lines: &[InfoLine],
    art_width: usize,
) -> String {
    let info_start = art_lines.len().saturating_sub(info_lines.len()) / 2;
    let mut output = String::new();
    for (idx, art_line) in art_lines.iter().enumerate() {
        output.push_str(&purple_gradient_line(art_line, idx, art_lines.len()));
        if let Some(info) = idx
            .checked_sub(info_start)
            .and_then(|line| info_lines.get(line))
        {
            let padding = art_width.saturating_sub(visible_width(art_line)) + SPLASH_GAP;
            output.push_str(&" ".repeat(padding));
            output.push_str(&info.render());
        }
        output.push('\n');
    }
    output
}

fn render_splash_stacked(art_lines: &[&str], info_lines: &[InfoLine]) -> String {
    let mut output = String::new();
    for (idx, art_line) in art_lines.iter().enumerate() {
        output.push_str(&purple_gradient_line(art_line, idx, art_lines.len()));
        output.push('\n');
    }
    output.push('\n');
    for info in info_lines {
        output.push_str(&info.render());
        output.push('\n');
    }
    output
}

fn render_splash_compact(info_lines: &[InfoLine]) -> String {
    let mut output = String::new();
    for info in info_lines {
        output.push_str(&info.render());
        output.push('\n');
    }
    output
}

/// A single line in the splash side panel, rendered with a fixed style.
enum InfoLine {
    /// Bright-orange heading.
    Head(String),
    /// Bold heading for the product name.
    Title(String),
    /// Dimmed body text (commands, values, paths).
    Body(String),
    Blank,
}

impl InfoLine {
    fn render(&self) -> String {
        match self {
            InfoLine::Head(text) => purple(text),
            InfoLine::Title(text) => purple_bold(text),
            InfoLine::Body(text) => dim(text),
            InfoLine::Blank => String::new(),
        }
    }

    /// Visible (uncoloured) width of the line, used to size the splash layout.
    fn visible_len(&self) -> usize {
        match self {
            InfoLine::Head(text) | InfoLine::Title(text) | InfoLine::Body(text) => {
                text.chars().count()
            }
            InfoLine::Blank => 0,
        }
    }
}

/// Side-panel content for the splash: branding plus the command menu. New
/// commands get a row here as they are added.
fn panel_info_lines() -> Vec<InfoLine> {
    let command = |name: &str, desc: &str| InfoLine::Body(format!("  {name:<16}{desc}"));
    vec![
        InfoLine::Title("Skopos".to_string()),
        InfoLine::Body("local-first AI usage observability".to_string()),
        InfoLine::Blank,
        InfoLine::Head("Commands".to_string()),
        command("help", "list commands"),
        command("work", "pick a project, launch CLI"),
        command("usage", "5h / weekly limit bars"),
        command("network", "connectivity dashboard"),
        command("claude -t/-w/-m", "usage by period"),
        command("claude models", "usage by model"),
        command("codex usage", "Codex 5h / weekly limits"),
        command("providers", "tracked providers"),
        command("claude import", "import Claude logs"),
        command("codex import", "import Codex rollout JSONLs"),
        command("gemini import", "import Gemini session JSONLs"),
        command("clear", "redraw splash"),
        command("exit", "quit skopos"),
        InfoLine::Blank,
        InfoLine::Head("Data".to_string()),
        InfoLine::Body("  ~/.local/share/skopos/skopos.db".to_string()),
    ]
}

fn purple_gradient_line(line: &str, index: usize, total_lines: usize) -> String {
    let denominator = total_lines.saturating_sub(1).max(1) as f32;
    let t = index as f32 / denominator;
    let start = (216.0, 180.0, 254.0);
    let end = (76.0, 29.0, 149.0);
    let r = lerp(start.0, end.0, t).round() as u8;
    let g = lerp(start.1, end.1, t).round() as u8;
    let b = lerp(start.2, end.2, t).round() as u8;
    rgb(line, (r, g, b))
}

fn lerp(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

fn visible_width(text: &str) -> usize {
    text.chars().count()
}
