//! Render `assets/skopos-ascii.txt` to a PNG with the same purple
//! vertical gradient the splash uses, on a neutral gray background.
//!
//! Run from anywhere inside the workspace:
//!
//! ```sh
//! cargo run --release -p skopos-cli --example render_ascii_png
//! ```
//!
//! Optional overrides via env vars:
//!
//! ```sh
//! SKOPOS_PNG_SIZE=4096           # output side in pixels (default 2048)
//! SKOPOS_PNG_OUT=path/to.png     # output file (default ./skopos-ascii.png)
//! SKOPOS_PNG_FONT=/path/Mono.ttf # override font autodetection
//! ```
//!
//! The gradient stops and the gray background match the splash code
//! 1:1 — see `src/splash.rs::purple_gradient_line`.

use std::path::{Path, PathBuf};

use ab_glyph::{FontArc, PxScale};
use anyhow::{Context, Result};
use image::{Rgb, RgbImage};
use imageproc::drawing::draw_text_mut;

/// Where the curated ASCII art lives, relative to this crate's
/// manifest. Resolved at compile time so the example works no matter
/// what the current working directory is.
const ASCII_RELATIVE: &str = "assets/skopos-ascii.txt";

/// Background — neutral mid-gray, same value used in the README/brand.
const BG: Rgb<u8> = Rgb([0x2A, 0x2A, 0x2E]);

/// Top gradient stop — lavanda claro. Matches `splash.rs`.
const GRADIENT_TOP: (f32, f32, f32) = (216.0, 180.0, 254.0);
/// Bottom gradient stop — violeta profundo. Matches `splash.rs`.
const GRADIENT_BOT: (f32, f32, f32) = (76.0, 29.0, 149.0);

/// Fraction of the canvas the ASCII bounding box should occupy. The
/// rest is symmetric margin so the piece breathes.
const ART_FRACTION: f32 = 0.78;

/// Monospace advance ratio. Real TTF advance varies per font, but we
/// draw each character on its own grid cell, so this only needs to be
/// "close enough" for the cell width to look like a faithful
/// monospace rendering. Most programming fonts sit at ~0.6.
const ADVANCE_RATIO: f32 = 0.6;

fn main() -> Result<()> {
    let size: u32 = std::env::var("SKOPOS_PNG_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2048);
    let out = PathBuf::from(
        std::env::var("SKOPOS_PNG_OUT").unwrap_or_else(|_| "skopos-ascii.png".into()),
    );

    let ascii_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(ASCII_RELATIVE);
    let art = std::fs::read_to_string(&ascii_path)
        .with_context(|| format!("read ASCII art {}", ascii_path.display()))?;
    let lines: Vec<&str> = art.lines().collect();
    let total_lines = lines.len();
    let max_cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1);

    let font_path = resolve_font()?;
    let font_bytes =
        std::fs::read(&font_path).with_context(|| format!("read font {}", font_path.display()))?;
    let font = FontArc::try_from_vec(font_bytes).context("parse font")?;

    // Pick the largest font size where both axes of the art's bounding
    // box still fit `ART_FRACTION * canvas`. Whichever axis is tighter
    // wins; the other axis just gets extra margin.
    let canvas = size as f32;
    let budget = canvas * ART_FRACTION;
    let line_h_from_rows = budget / total_lines as f32;
    let line_h_from_cols = budget / (max_cols as f32 * ADVANCE_RATIO);
    let line_h = line_h_from_rows.min(line_h_from_cols);
    let scale = PxScale::from(line_h);
    let advance = line_h * ADVANCE_RATIO;

    let art_w = max_cols as f32 * advance;
    let art_h = total_lines as f32 * line_h;
    let origin_x = (canvas - art_w) / 2.0;
    let origin_y = (canvas - art_h) / 2.0;

    let mut img = RgbImage::from_pixel(size, size, BG);
    let mut char_buf = [0u8; 4];

    for (row, line) in lines.iter().enumerate() {
        let color = Rgb(gradient_at(row, total_lines));
        let y = (origin_y + row as f32 * line_h).round() as i32;
        for (col, ch) in line.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let x = (origin_x + col as f32 * advance).round() as i32;
            // Render each character independently so the column grid
            // is exactly `col * advance` — that keeps the geometry of
            // the ASCII art faithful regardless of the font's own
            // per-glyph advance metrics.
            let s = ch.encode_utf8(&mut char_buf);
            draw_text_mut(&mut img, color, x, y, scale, &font, s);
        }
    }

    img.save(&out)
        .with_context(|| format!("write {}", out.display()))?;
    println!(
        "wrote {} ({size}x{size}, font {})",
        out.display(),
        font_path.display(),
    );
    Ok(())
}

/// Linear interpolation of the gradient stops, identical to
/// `splash::purple_gradient_line` so the PNG matches the terminal.
fn gradient_at(index: usize, total: usize) -> [u8; 3] {
    let denom = total.saturating_sub(1).max(1) as f32;
    let t = index as f32 / denom;
    [
        lerp(GRADIENT_TOP.0, GRADIENT_BOT.0, t).round() as u8,
        lerp(GRADIENT_TOP.1, GRADIENT_BOT.1, t).round() as u8,
        lerp(GRADIENT_TOP.2, GRADIENT_BOT.2, t).round() as u8,
    ]
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Locate a monospace TTF the example can render with. `SKOPOS_PNG_FONT`
/// always wins; otherwise we probe the common install paths on
/// Arch / Debian / Fedora / macOS in preference order, picking the
/// font whose glyph coverage includes π, ∞ and ≠ (the non-ASCII
/// characters in the art). DejaVu Sans Mono and JetBrains Mono both
/// satisfy this.
fn resolve_font() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("SKOPOS_PNG_FONT") {
        return Ok(PathBuf::from(path));
    }
    const CANDIDATES: &[&str] = &[
        // Arch
        "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Regular.ttf",
        "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Regular.ttf",
        "/usr/share/fonts/TTF/JetBrainsMono-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        "/usr/share/fonts/TTF/FiraCode-Regular.ttf",
        // Debian / Ubuntu
        "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        // Fedora
        "/usr/share/fonts/jetbrains-mono-fonts/JetBrainsMono-Regular.ttf",
        "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
        // macOS (bundled)
        "/System/Library/Fonts/Menlo.ttc",
        "/Library/Fonts/Menlo.ttc",
    ];
    for candidate in CANDIDATES {
        if Path::new(candidate).is_file() {
            return Ok(PathBuf::from(candidate));
        }
    }
    anyhow::bail!("no monospace font found in known locations; set SKOPOS_PNG_FONT=<path-to-ttf>")
}
