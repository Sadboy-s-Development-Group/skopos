//! Generate the systemd unit that runs `skopos network watch`.
//!
//! A per-user unit (`--user`) is written straight into
//! `~/.config/systemd/user/` and can be enabled with no root. A system
//! unit is staged into the Skopos data dir and the root commands to
//! install it are printed — Skopos never edits `/etc` itself.

use std::fs;
use std::path::PathBuf;

use crate::install::skopos_binary_path;
use crate::limits::home_dir;
use crate::{dim, purple, purple_bold};

const UNIT_NAME: &str = "skopos-netwatch.service";

/// Render the systemd unit. `system` units pin `User=` and target
/// `multi-user.target`; user units target `default.target`.
fn unit_text(system: bool) -> String {
    let exec = format!("{} network watch", skopos_binary_path().display());
    let mut text = String::new();
    text.push_str("[Unit]\n");
    text.push_str("Description=Skopos network watch — internet connectivity probe\n");
    text.push_str("After=network-online.target\n");
    text.push_str("Wants=network-online.target\n\n");
    text.push_str("[Service]\n");
    text.push_str("Type=simple\n");
    text.push_str(&format!("ExecStart={exec}\n"));
    text.push_str("Restart=always\n");
    text.push_str("RestartSec=5\n");
    if system {
        if let Some(user) = current_user() {
            text.push_str(&format!("User={user}\n"));
        }
    }
    text.push('\n');
    text.push_str("[Install]\n");
    text.push_str(if system {
        "WantedBy=multi-user.target\n"
    } else {
        "WantedBy=default.target\n"
    });
    text
}

fn current_user() -> Option<String> {
    std::env::var("USER").ok().filter(|user| !user.is_empty())
}

fn user_unit_path() -> PathBuf {
    home_dir()
        .join(".config")
        .join("systemd")
        .join("user")
        .join(UNIT_NAME)
}

fn system_unit_staging_path() -> PathBuf {
    home_dir()
        .join(".local")
        .join("share")
        .join("skopos")
        .join(UNIT_NAME)
}

pub(super) fn run_install(user: bool) -> anyhow::Result<String> {
    let mut out = String::new();
    out.push_str(&purple_bold("Install network watch service"));
    out.push_str("\n\n");

    if user {
        let path = user_unit_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, unit_text(false))?;
        out.push_str(&purple("  wrote per-user unit.\n"));
        out.push_str(&dim(&format!("  unit:  {}\n", path.display())));
        out.push('\n');
        out.push_str(&dim("  enable it with:\n"));
        out.push_str("    systemctl --user daemon-reload\n");
        out.push_str(&format!("    systemctl --user enable --now {UNIT_NAME}\n"));
        out.push('\n');
        out.push_str(&dim(
            "  on a headless server, keep it running across logout:\n",
        ));
        out.push_str("    sudo loginctl enable-linger $USER\n");
    } else {
        let staged = system_unit_staging_path();
        if let Some(parent) = staged.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&staged, unit_text(true))?;
        out.push_str(&purple("  staged system unit.\n"));
        out.push_str(&dim(&format!("  draft: {}\n", staged.display())));
        out.push('\n');
        out.push_str(&dim("  install it as root with:\n"));
        out.push_str(&format!(
            "    sudo cp {} /etc/systemd/system/{UNIT_NAME}\n",
            staged.display(),
        ));
        out.push_str("    sudo systemctl daemon-reload\n");
        out.push_str(&format!("    sudo systemctl enable --now {UNIT_NAME}\n"));
        out.push('\n');
        out.push_str(&dim(
            "  or re-run with --user for a no-root per-user service.\n",
        ));
    }
    Ok(out)
}

pub(super) fn run_uninstall(user: bool) -> anyhow::Result<String> {
    let mut out = String::new();
    out.push_str(&purple_bold("Uninstall network watch service"));
    out.push_str("\n\n");

    if user {
        let path = user_unit_path();
        out.push_str(&dim("  disable it first with:\n"));
        out.push_str(&format!("    systemctl --user disable --now {UNIT_NAME}\n"));
        out.push('\n');
        if path.exists() {
            fs::remove_file(&path)?;
            out.push_str(&purple(&format!("  removed {}\n", path.display())));
        } else {
            out.push_str(&dim("  no per-user unit file found.\n"));
        }
    } else {
        out.push_str(&dim("  remove it as root with:\n"));
        out.push_str(&format!("    sudo systemctl disable --now {UNIT_NAME}\n"));
        out.push_str(&format!("    sudo rm /etc/systemd/system/{UNIT_NAME}\n"));
        out.push_str("    sudo systemctl daemon-reload\n");
    }
    Ok(out)
}
