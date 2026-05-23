//! Apply a terminal-style profile.
//!
//! Three side-effects, all idempotent, mirroring the original
//! `logo-switch.sh` 1:1 so the user can swap between the two without
//! divergence:
//!
//! 1. Re-point `<fastfetch_root>/logo.txt` (symlink) at the profile's logo.
//! 2. Patch `<fastfetch_root>/config.jsonc` — three colour fields under
//!    `display.color` (`title`, `keys`) plus the separator `outputColor`.
//! 3. Replace the `"palette": { … }` block inside
//!    `~/.config/oh-my-posh/theme.omp.json`.
//!
//! Each text file is written via `tmp + rename` so a crash mid-edit can
//! never leave the user with a half-written config.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use super::profile::{Palette, Profile};

/// Outcome of an apply run, surfaced to whoever called us so they can
/// print a one-line summary.
#[derive(Debug, Default)]
pub(crate) struct ApplyReport {
    pub omp_skipped: bool,
}

pub(crate) fn apply(fastfetch_root: &Path, profile: &Profile) -> Result<ApplyReport> {
    relink_logo(fastfetch_root, profile)?;
    patch_fastfetch_config(&fastfetch_root.join("config.jsonc"), &profile.palette)?;
    let omp = omp_theme_path();
    let omp_skipped = if omp.exists() {
        patch_omp_palette(&omp, &profile.palette)?;
        false
    } else {
        true
    };
    Ok(ApplyReport { omp_skipped })
}

fn omp_theme_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("oh-my-posh").join("theme.omp.json");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".config")
        .join("oh-my-posh")
        .join("theme.omp.json")
}

fn relink_logo(fastfetch_root: &Path, profile: &Profile) -> Result<()> {
    let active = fastfetch_root.join("logo.txt");
    // The script uses a relative target so `config.jsonc` can keep
    // `"source": "logo.txt"`. Match that.
    let target: PathBuf = PathBuf::from("profiles")
        .join(&profile.name)
        .join("logo.txt");
    if active.exists() || active.symlink_metadata().is_ok() {
        fs::remove_file(&active).with_context(|| format!("removing {}", active.display()))?;
    }
    symlink(&target, &active)
        .with_context(|| format!("symlinking {} -> {}", active.display(), target.display()))?;
    Ok(())
}

pub(crate) fn patch_fastfetch_config(path: &Path, palette: &Palette) -> Result<()> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut out = raw.clone();
    out = replace_first_string_field(&out, "title", &palette.ff_title)
        .ok_or_else(|| anyhow!("'title' field not found in {}", path.display()))?;
    out = replace_first_string_field(&out, "keys", &palette.ff_keys)
        .ok_or_else(|| anyhow!("'keys' field not found in {}", path.display()))?;
    out = replace_first_string_field(&out, "outputColor", &palette.ff_sep)
        .ok_or_else(|| anyhow!("'outputColor' field not found in {}", path.display()))?;
    if out != raw {
        atomic_write(path, &out)?;
    }
    Ok(())
}

/// Replace the value of the first `"<field>": "<...>"` occurrence with
/// `new`. Returns `None` if no such field exists. Whitespace between the
/// colon and the opening quote is preserved; the surrounding text isn't
/// touched, which is what we want for JSONC (it keeps comments intact).
pub(crate) fn replace_first_string_field(text: &str, field: &str, new: &str) -> Option<String> {
    let needle = format!("\"{}\"", field);
    let mut cursor = 0;
    while let Some(rel) = text[cursor..].find(&needle) {
        let key_start = cursor + rel;
        let after_key = key_start + needle.len();
        // Skip whitespace then expect `:`.
        let mut i = after_key;
        while i < text.len() && text.as_bytes()[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= text.len() || text.as_bytes()[i] != b':' {
            cursor = after_key;
            continue;
        }
        i += 1;
        while i < text.len() && text.as_bytes()[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= text.len() || text.as_bytes()[i] != b'"' {
            cursor = after_key;
            continue;
        }
        let val_start = i + 1;
        let mut j = val_start;
        while j < text.len() && text.as_bytes()[j] != b'"' {
            if text.as_bytes()[j] == b'\\' && j + 1 < text.len() {
                j += 2;
            } else {
                j += 1;
            }
        }
        if j >= text.len() {
            return None;
        }
        let mut result = String::with_capacity(text.len() + new.len());
        result.push_str(&text[..val_start]);
        result.push_str(new);
        result.push_str(&text[j..]);
        return Some(result);
    }
    None
}

pub(crate) fn patch_omp_palette(path: &Path, palette: &Palette) -> Result<()> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let new = replace_palette_block(&raw, palette).ok_or_else(|| {
        anyhow!(
            "could not find a `\"palette\": {{...}}` block in {}",
            path.display()
        )
    })?;
    if new != raw {
        atomic_write(path, &new)?;
    }
    Ok(())
}

/// Find the first `"palette": { ... }` block (matching braces) and
/// replace its contents with the five hex entries from `palette`. Two-space
/// inner indent is used to match the layout in the bundled theme.
pub(crate) fn replace_palette_block(text: &str, palette: &Palette) -> Option<String> {
    let key = "\"palette\"";
    let start = text.find(key)?;
    let after_key = start + key.len();
    let bytes = text.as_bytes();
    let mut i = after_key;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b':' {
        return None;
    }
    i += 1;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'{' {
        return None;
    }
    let block_start = i;
    let mut depth = 0i32;
    let mut j = i;
    while j < bytes.len() {
        match bytes[j] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    j += 1;
                    break;
                }
            }
            _ => {}
        }
        j += 1;
    }
    if depth != 0 {
        return None;
    }
    let replacement = format!(
        "{{\n        \"os\": \"{os}\",\n        \"session\": \"{se}\",\n        \"path\": \"{pa}\",\n        \"git\": \"{gi}\",\n        \"closer\": \"{cl}\"\n  }}",
        os = palette.omp_os,
        se = palette.omp_session,
        pa = palette.omp_path,
        gi = palette.omp_git,
        cl = palette.omp_closer,
    );
    let mut out = String::with_capacity(text.len() + replacement.len());
    out.push_str(&text[..block_start]);
    out.push_str(&replacement);
    out.push_str(&text[j..]);
    Some(out)
}

fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("path has no parent: {}", path.display()))?;
    if !parent.exists() {
        bail!("parent directory missing: {}", parent.display());
    }
    let tmp = parent.join(format!(
        ".{}.skopos-tmp",
        path.file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_palette() -> Palette {
        Palette {
            omp_os: "#89DCEB".into(),
            omp_session: "#74C7EC".into(),
            omp_path: "#94E2D5".into(),
            omp_git: "#89B4FA".into(),
            omp_closer: "#74C7EC".into(),
            ff_title: "38;2;137;220;235".into(),
            ff_keys: "38;2;116;199;236".into(),
            ff_sep: "38;2;116;199;236".into(),
        }
    }

    #[test]
    fn replaces_string_field_preserving_comments() {
        let src = r#"{
  // header
  "display": {
    "color": {
      "title": "38;2;0;0;0",  // old
      "keys": "38;2;1;1;1"
    }
  }
}"#;
        let out = replace_first_string_field(src, "title", "38;2;9;9;9").unwrap();
        assert!(out.contains("\"title\": \"38;2;9;9;9\""));
        assert!(out.contains("// header"));
        assert!(out.contains("// old"));
        // The 'keys' field must remain untouched by a 'title' rewrite.
        assert!(out.contains("\"keys\": \"38;2;1;1;1\""));
    }

    #[test]
    fn fastfetch_patch_is_idempotent() {
        let src = r#"{
  "display": {
    "color": { "title": "old", "keys": "old" }
  },
  "modules": [
    { "type": "separator", "outputColor": "old" }
  ]
}"#;
        let tmp = tempfile_path("fastfetch-cfg.jsonc");
        std::fs::write(&tmp, src).unwrap();
        patch_fastfetch_config(&tmp, &sample_palette()).unwrap();
        let once = std::fs::read_to_string(&tmp).unwrap();
        patch_fastfetch_config(&tmp, &sample_palette()).unwrap();
        let twice = std::fs::read_to_string(&tmp).unwrap();
        assert_eq!(once, twice);
        assert!(once.contains("\"title\": \"38;2;137;220;235\""));
        assert!(once.contains("\"keys\": \"38;2;116;199;236\""));
        assert!(once.contains("\"outputColor\": \"38;2;116;199;236\""));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn replaces_omp_palette_block() {
        let src = r##"{
  "$schema": "x",
  "palette": {
        "os": "#000000",
        "session": "#000000",
        "path": "#000000",
        "git": "#000000",
        "closer": "#000000"
  },
  "blocks": []
}"##;
        let out = replace_palette_block(src, &sample_palette()).unwrap();
        assert!(out.contains("\"os\": \"#89DCEB\""));
        assert!(out.contains("\"session\": \"#74C7EC\""));
        assert!(out.contains("\"path\": \"#94E2D5\""));
        assert!(out.contains("\"git\": \"#89B4FA\""));
        assert!(out.contains("\"closer\": \"#74C7EC\""));
        // Surrounding content survived.
        assert!(out.contains("\"blocks\": []"));
        assert!(out.contains("\"$schema\": \"x\""));
        // And applying twice is a no-op (idempotent).
        let twice = replace_palette_block(&out, &sample_palette()).unwrap();
        assert_eq!(out, twice);
    }

    fn tempfile_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        let pid = std::process::id();
        let nano = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("skopos-test-{pid}-{nano}-{name}"));
        p
    }
}
