//! Shared terminal styling.
//!
//! Every Skopos renderer builds its coloured output from the same two
//! primitives ([`rgb`] / [`rgb_bold`]) and the same named brand palette,
//! so the ANSI escape sequence is written in exactly one place.

/// Skopos brand purple — side-panel text, table headers, labels.
pub(crate) const PURPLE: (u8, u8, u8) = (189, 147, 249);

/// Anthropic brand orange — the fill colour for Claude rate-limit bars.
pub(crate) const ANTHROPIC_ORANGE: (u8, u8, u8) = (204, 120, 50);

/// Codex accent green — the fill colour for Codex rate-limit bars.
pub(crate) const CODEX_GREEN: (u8, u8, u8) = (180, 220, 130);

/// Dimmed grey — secondary text, paths, hints.
pub(crate) const GREY: (u8, u8, u8) = (140, 140, 140);

/// Truecolor foreground text.
pub(crate) fn rgb(text: &str, (r, g, b): (u8, u8, u8)) -> String {
    format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
}

/// Bold truecolor foreground text.
pub(crate) fn rgb_bold(text: &str, (r, g, b): (u8, u8, u8)) -> String {
    format!("\x1b[1m\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
}

/// Bright-purple foreground text.
pub(crate) fn purple(text: &str) -> String {
    rgb(text, PURPLE)
}

/// Bold bright-purple foreground text.
pub(crate) fn purple_bold(text: &str) -> String {
    rgb_bold(text, PURPLE)
}

/// Dimmed grey foreground text.
pub(crate) fn dim(text: &str) -> String {
    rgb(text, GREY)
}

/// Anthropic-orange foreground text — Claude bars, regardless of fill.
pub(crate) fn anthropic_orange(text: &str) -> String {
    rgb(text, ANTHROPIC_ORANGE)
}

/// Codex-green foreground text — Codex bars, regardless of fill.
pub(crate) fn codex_green(text: &str) -> String {
    rgb(text, CODEX_GREEN)
}
