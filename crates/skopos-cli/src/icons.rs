//! Project-type detection for the `skopos work` picker.
//!
//! Each project directory is inspected for marker files and mapped to a
//! Nerd Font glyph plus an accent colour. Glyphs come from the Nerd Font
//! Private Use Area (U+E000–U+F8FF) and require a Nerd Font in the
//! terminal — we already detected one on the user's machine.
//!
//! Detection is intentionally tiny and ordered: the first matching rule
//! wins. Add a rule by extending [`detect`].

use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProjectIcon {
    pub glyph: &'static str,
    pub color: (u8, u8, u8),
}

/// Plain folder, used when no language marker matches.
const FOLDER: ProjectIcon = ProjectIcon {
    glyph: "\u{f07b}", // nf-fa-folder
    color: (140, 140, 140),
};

/// Pick an icon for `project` based on the files it contains.
pub(crate) fn detect(project: &Path) -> ProjectIcon {
    if project.join("Cargo.toml").is_file() {
        return ProjectIcon {
            glyph: "\u{e7a8}", // nf-dev-rust
            color: (222, 165, 132),
        };
    }
    if project.join("package.json").is_file() {
        if project.join("tsconfig.json").is_file() {
            return ProjectIcon {
                glyph: "\u{e628}", // nf-seti-typescript
                color: (49, 120, 198),
            };
        }
        return ProjectIcon {
            glyph: "\u{e781}", // nf-seti-javascript
            color: (240, 219, 79),
        };
    }
    if project.join("pyproject.toml").is_file()
        || project.join("requirements.txt").is_file()
        || project.join("setup.py").is_file()
    {
        return ProjectIcon {
            glyph: "\u{e73c}", // nf-dev-python
            color: (247, 212, 76),
        };
    }
    if project.join("go.mod").is_file() {
        return ProjectIcon {
            glyph: "\u{e627}", // nf-seti-go
            color: (0, 173, 216),
        };
    }
    if project.join("Gemfile").is_file() {
        return ProjectIcon {
            glyph: "\u{e791}", // nf-dev-ruby
            color: (224, 38, 38),
        };
    }
    if project.join(".git").exists() {
        return ProjectIcon {
            glyph: "\u{e702}", // nf-dev-git
            color: (175, 175, 175),
        };
    }
    FOLDER
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("skopos-icons-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_rust_from_cargo_toml() {
        let dir = tempdir("rust");
        fs::write(dir.join("Cargo.toml"), "").unwrap();
        assert_eq!(detect(&dir).glyph, "\u{e7a8}");
    }

    #[test]
    fn detects_typescript_when_tsconfig_present() {
        let dir = tempdir("ts");
        fs::write(dir.join("package.json"), "{}").unwrap();
        fs::write(dir.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(detect(&dir).glyph, "\u{e628}");
    }

    #[test]
    fn falls_back_to_folder() {
        let dir = tempdir("empty");
        assert_eq!(detect(&dir).glyph, FOLDER.glyph);
    }
}
