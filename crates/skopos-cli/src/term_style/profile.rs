//! Terminal-style profile discovery + `palette.sh` parsing.
//!
//! Profiles live under `$XDG_CONFIG_HOME/fastfetch/profiles/<name>/`, each
//! with a `palette.sh` (key=value bash assignments) and a `logo.txt`
//! (pre-rendered ASCII with truecolor escapes). The active profile is the
//! one `<fastfetch_root>/logo.txt` symlinks to.
//!
//! We *parse* `palette.sh`, never source it — the file is data, not code.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

const REQUIRED_KEYS: &[&str] = &[
    "OMP_OS",
    "OMP_SESSION",
    "OMP_PATH",
    "OMP_GIT",
    "OMP_CLOSER",
    "FF_TITLE",
    "FF_KEYS",
    "FF_SEP",
];

#[derive(Debug, Clone)]
pub(crate) struct Palette {
    pub omp_os: String,
    pub omp_session: String,
    pub omp_path: String,
    pub omp_git: String,
    pub omp_closer: String,
    pub ff_title: String,
    pub ff_keys: String,
    pub ff_sep: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Profile {
    pub name: String,
    pub palette: Palette,
    pub logo: String,
}

/// Result of scanning the profiles directory: every profile that parsed
/// cleanly, plus the name of the currently active one (if any).
#[derive(Debug)]
pub(crate) struct Discovery {
    pub fastfetch_root: PathBuf,
    pub profiles: Vec<Profile>,
    pub active: Option<String>,
}

pub(crate) fn discover() -> Result<Discovery> {
    let root = fastfetch_root();
    let profiles_dir = root.join("profiles");
    if !profiles_dir.is_dir() {
        return Err(anyhow!(
            "no profiles directory at {} — set up at least one profile under ~/.config/fastfetch/profiles/",
            profiles_dir.display()
        ));
    }

    let mut profiles = Vec::new();
    for entry in fs::read_dir(&profiles_dir)
        .with_context(|| format!("reading {}", profiles_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        match load_profile(&name, &dir) {
            Ok(p) => profiles.push(p),
            Err(e) => {
                eprintln!("skopos term-style: skipping profile '{}': {:#}", name, e);
            }
        }
    }
    profiles.sort_by(|a, b| a.name.cmp(&b.name));

    let active = detect_active(&root, &profiles);
    Ok(Discovery {
        fastfetch_root: root,
        profiles,
        active,
    })
}

fn fastfetch_root() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("fastfetch");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".config")
        .join("fastfetch")
}

fn load_profile(name: &str, dir: &Path) -> Result<Profile> {
    let palette_path = dir.join("palette.sh");
    let logo_path = dir.join("logo.txt");
    let palette = parse_palette_file(&palette_path)
        .with_context(|| format!("reading {}", palette_path.display()))?;
    let logo = fs::read_to_string(&logo_path)
        .with_context(|| format!("reading {}", logo_path.display()))?;
    Ok(Profile {
        name: name.to_string(),
        palette,
        logo,
    })
}

fn parse_palette_file(path: &Path) -> Result<Palette> {
    let raw = fs::read_to_string(path)?;
    let map = parse_palette_str(&raw);
    fn get(map: &HashMap<String, String>, key: &str) -> Result<String> {
        map.get(key)
            .cloned()
            .ok_or_else(|| anyhow!("missing {key}"))
    }
    for k in REQUIRED_KEYS {
        if !map.contains_key(*k) {
            return Err(anyhow!("missing {k}"));
        }
    }
    Ok(Palette {
        omp_os: get(&map, "OMP_OS")?,
        omp_session: get(&map, "OMP_SESSION")?,
        omp_path: get(&map, "OMP_PATH")?,
        omp_git: get(&map, "OMP_GIT")?,
        omp_closer: get(&map, "OMP_CLOSER")?,
        ff_title: get(&map, "FF_TITLE")?,
        ff_keys: get(&map, "FF_KEYS")?,
        ff_sep: get(&map, "FF_SEP")?,
    })
}

/// Parse a `KEY="value"` / `KEY=value` bash file into a flat map. Comments
/// (`#`) and blank lines are skipped; nothing else is interpreted.
pub(crate) fn parse_palette_str(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in raw.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let value = strip_value(rest);
        out.insert(key.to_string(), value);
    }
    out
}

/// Strip surrounding quotes and trailing comments from a bash RHS.
fn strip_value(raw: &str) -> String {
    let raw = raw.trim_start();
    let (body, _) = if let Some(rest) = raw.strip_prefix('"') {
        // Quoted: take everything up to the next unescaped `"`.
        match rest.find('"') {
            Some(end) => (&rest[..end], &rest[end + 1..]),
            None => (rest, ""),
        }
    } else if let Some(rest) = raw.strip_prefix('\'') {
        match rest.find('\'') {
            Some(end) => (&rest[..end], &rest[end + 1..]),
            None => (rest, ""),
        }
    } else {
        // Bare: stop at first whitespace (which also ends inline comments,
        // since `#` is only a comment when it follows whitespace — that
        // way a leading `#` like `#74C7EC` stays in the value).
        let end = raw.find(|c: char| c.is_whitespace()).unwrap_or(raw.len());
        (&raw[..end], &raw[end..])
    };
    body.to_string()
}

/// Identify the active profile by resolving the symlink that `logo-switch.sh`
/// writes. Falls back to byte-level comparison of `logo.txt` against each
/// profile's logo when the file is a regular copy rather than a symlink.
fn detect_active(root: &Path, profiles: &[Profile]) -> Option<String> {
    let active_logo = root.join("logo.txt");
    if let Ok(target) = fs::read_link(&active_logo) {
        let absolute = if target.is_absolute() {
            target.clone()
        } else {
            root.join(&target)
        };
        // Expected layout: <root>/profiles/<name>/logo.txt
        if let Some(parent) = absolute.parent() {
            if let Some(name) = parent.file_name() {
                let name = name.to_string_lossy();
                if profiles.iter().any(|p| p.name == name) {
                    return Some(name.into_owned());
                }
            }
        }
    }
    if let Ok(current) = fs::read_to_string(&active_logo) {
        return profiles
            .iter()
            .find(|p| p.logo == current)
            .map(|p| p.name.clone());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_palette_handles_comments_quotes_and_bare_values() {
        let raw = r##"
# Perfil "allay"
OMP_OS="#89DCEB"        # Sky      — icono OS
OMP_SESSION="#74C7EC"
OMP_PATH=  "#94E2D5"
OMP_GIT='#89B4FA'
OMP_CLOSER=#74C7EC      # bare
FF_TITLE="38;2;137;220;235"
FF_KEYS="38;2;116;199;236"
FF_SEP="38;2;116;199;236"
"##;
        let m = parse_palette_str(raw);
        assert_eq!(m.get("OMP_OS").unwrap(), "#89DCEB");
        assert_eq!(m.get("OMP_SESSION").unwrap(), "#74C7EC");
        assert_eq!(m.get("OMP_PATH").unwrap(), "#94E2D5");
        assert_eq!(m.get("OMP_GIT").unwrap(), "#89B4FA");
        assert_eq!(m.get("OMP_CLOSER").unwrap(), "#74C7EC");
        assert_eq!(m.get("FF_TITLE").unwrap(), "38;2;137;220;235");
    }

    #[test]
    fn parse_palette_skips_garbage_lines() {
        let raw = "garbage\nNOT VALID KEY=foo\nGOOD=bar\n";
        let m = parse_palette_str(raw);
        assert_eq!(m.get("GOOD").unwrap(), "bar");
        assert!(!m.contains_key("NOT VALID KEY"));
    }
}
