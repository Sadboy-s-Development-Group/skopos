//! `skopos term-style` — switch the terminal style (fastfetch logo +
//! colours + oh-my-posh palette) across the profiles the user maintains
//! under `~/.config/fastfetch/profiles/`. With no profile name, a TUI
//! picker opens; with a name, the apply runs headless and exits.

mod apply;
mod profile;
mod tui;

use std::io::IsTerminal;

use anyhow::{anyhow, Result};

pub(crate) fn run(name: Option<String>) -> Result<()> {
    let discovery = profile::discover()?;

    if let Some(name) = name {
        let profile = discovery
            .profiles
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| {
                let available: Vec<&str> =
                    discovery.profiles.iter().map(|p| p.name.as_str()).collect();
                anyhow!("no profile '{name}'. Available: {}", available.join(", "))
            })?;
        let report = apply::apply(&discovery.fastfetch_root, profile)?;
        println!(
            "Active profile: {} — open a new terminal or run 'fastfetch' to see it.",
            profile.name
        );
        if report.omp_skipped {
            println!("  (oh-my-posh theme.omp.json not found; only fastfetch was updated.)");
        }
        return Ok(());
    }

    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        // Not a TTY: list profiles instead of trying to open the TUI.
        list_profiles(&discovery);
        return Ok(());
    }
    tui::run(discovery)
}

fn list_profiles(d: &profile::Discovery) {
    for p in &d.profiles {
        let marker = if d.active.as_deref() == Some(p.name.as_str()) {
            "*"
        } else {
            " "
        };
        println!("{marker} {}", p.name);
    }
}
