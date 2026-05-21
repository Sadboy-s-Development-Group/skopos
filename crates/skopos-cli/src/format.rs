//! Presentation helpers shared by the usage reports: compact number
//! formatting, the aligned-table renderer, and the calendar windows the
//! period reports query over.

use chrono::{DateTime, Datelike, TimeZone, Utc};

use crate::theme::{dim, purple};

/// Which calendar window a period report covers.
#[derive(Debug, Clone, Copy)]
pub(crate) enum UsagePeriod {
    Today,
    Week,
    Month,
}

/// Compact human-readable token count, e.g. `250.5M`, `6.3K`, `512`.
pub(crate) fn human_tokens(n: i64) -> String {
    let value = n as f64;
    let abs = value.abs();
    if abs < 1_000.0 {
        n.to_string()
    } else if abs < 1_000_000.0 {
        format!("{:.1}K", value / 1_000.0)
    } else if abs < 1_000_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else {
        format!("{:.1}B", value / 1_000_000_000.0)
    }
}

/// Integer with thousands separators, e.g. `1,722`.
pub(crate) fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    for (idx, byte) in bytes.iter().enumerate() {
        if idx > 0 && (bytes.len() - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// Render an aligned table: column 0 left-aligned, the rest right-aligned.
/// The header row and underline are coloured; data rows stay plain.
pub(crate) fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let columns = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|header| header.len()).collect();
    for row in rows {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(cell.chars().count());
        }
    }

    let format_row = |cells: &[String]| -> String {
        let mut line = String::from("  ");
        for (idx, cell) in cells.iter().enumerate() {
            if idx == 0 {
                line.push_str(&format!("{:<width$}", cell, width = widths[idx]));
            } else {
                line.push_str(&format!("  {:>width$}", cell, width = widths[idx]));
            }
        }
        line
    };

    let header_cells: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
    let total_width: usize = widths.iter().sum::<usize>() + 2 + (columns - 1) * 2;

    let mut out = String::new();
    out.push_str(&purple(&format_row(&header_cells)));
    out.push('\n');
    out.push_str(&dim(&format!(
        "  {}",
        "─".repeat(total_width.saturating_sub(2))
    )));
    out.push('\n');
    for row in rows {
        out.push_str(&format_row(row));
        out.push('\n');
    }
    out
}

pub(crate) fn today_range(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = Utc
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .unwrap();
    (start, start + chrono::Duration::days(1))
}

pub(crate) fn week_range(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let (today_start, _) = today_range(now);
    let days_since_monday = now.weekday().num_days_from_monday() as i64;
    let start = today_start - chrono::Duration::days(days_since_monday);
    (start, start + chrono::Duration::days(7))
}

pub(crate) fn month_range(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = Utc
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .unwrap();
    let end = if now.month() == 12 {
        Utc.with_ymd_and_hms(now.year() + 1, 1, 1, 0, 0, 0).unwrap()
    } else {
        Utc.with_ymd_and_hms(now.year(), now.month() + 1, 1, 0, 0, 0)
            .unwrap()
    };
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_tokens_uses_compact_suffixes() {
        assert_eq!(human_tokens(512), "512");
        assert_eq!(human_tokens(6_316_399), "6.3M");
        assert_eq!(human_tokens(250_473_138), "250.5M");
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(1_722), "1,722");
        assert_eq!(thousands(100), "100");
        assert_eq!(thousands(1_822_000), "1,822,000");
    }
}
