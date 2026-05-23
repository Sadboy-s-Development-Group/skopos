//! `skopos update` — pull the latest GitHub Release and replace this binary.
//!
//! Wraps [`self_update`] in a Skopos-friendly façade: we always know the
//! repo (the `Sadboy-s-Development-Group/skopos` constants below), and we publish prebuilt
//! tarballs whose filenames embed the Rust target triple, so
//! `self_update`'s default asset matcher picks the right artifact by
//! looking for the running platform's target string. That keeps this
//! module honest — adding a new platform is a release-workflow concern,
//! not a CLI concern.
//!
//! `--check` short-circuits before any download so curious users can
//! see whether they are on the latest version without writing anything
//! to disk.

use std::fmt::Write as _;

use self_update::cargo_crate_version;

use crate::theme::{dim, purple_bold};

/// GitHub owner that publishes Skopos releases.
const REPO_OWNER: &str = "Sadboy-s-Development-Group";
/// GitHub repository name. Combined with `REPO_OWNER` to build the
/// releases API URL `self_update` queries.
const REPO_NAME: &str = "skopos";
/// Name of the binary inside the release archive. Must match the
/// `[[bin]] name` in skopos-cli/Cargo.toml and the name the release
/// workflow uses when bundling the tarball.
const BIN_NAME: &str = "skopos";

/// Render the `skopos update` report. With `check_only`, the binary on
/// disk is never touched — we print the latest release's version and
/// return.
pub(crate) fn run(check_only: bool) -> anyhow::Result<String> {
    let current = cargo_crate_version!();
    let mut out = String::new();
    writeln!(out, "{}", purple_bold("Skopos update"))?;
    writeln!(out)?;
    writeln!(out, "  current: v{current}")?;

    let target = self_update::get_target();
    writeln!(out, "  target:  {target}")?;
    writeln!(out)?;

    // While the repo is private, GitHub's anonymous `/releases` returns
    // 404; pulling a token out of the environment lets users on the
    // private beta hit the same code path that public users will use
    // once the repo flips. Empty token is treated as no token.
    let auth_token = std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty());

    let mut release_list = self_update::backends::github::ReleaseList::configure();
    release_list.repo_owner(REPO_OWNER).repo_name(REPO_NAME);
    if let Some(token) = auth_token.as_deref() {
        release_list.auth_token(token);
    }
    let release = release_list
        .build()?
        .fetch()
        // GitHub answers `/releases` with 404 when a repo has no
        // releases yet. That's a totally normal pre-first-tag state,
        // not a real network failure — translate it so the user
        // doesn't see a scary "NetworkError 404" verbatim.
        .map_err(|err| {
            if err.to_string().contains("404") {
                anyhow::anyhow!(
                    "no releases published for {REPO_OWNER}/{REPO_NAME} yet — try again later"
                )
            } else {
                anyhow::Error::new(err)
            }
        })?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no releases published for {REPO_OWNER}/{REPO_NAME} yet"))?;

    writeln!(out, "  latest:  {}", release.version)?;

    let already_latest = !is_newer(&release.version, current);
    if already_latest {
        writeln!(out)?;
        writeln!(out, "  {}", dim("already on the latest release."))?;
        return Ok(out);
    }

    if check_only {
        writeln!(out)?;
        writeln!(
            out,
            "  a newer release is available. run `skopos update` to install.",
        )?;
        return Ok(out);
    }

    writeln!(out)?;
    writeln!(out, "  downloading and replacing the binary in place…")?;

    let mut update_cfg = self_update::backends::github::Update::configure();
    update_cfg
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        // Release tarballs wrap the binary inside a `skopos-{version}-{target}/`
        // directory (see `.github/workflows/release.yml`). Without this,
        // `self_update` looks for the binary at the archive root and fails.
        .bin_path_in_archive("skopos-{{ version }}-{{ target }}/{{ bin }}")
        // Each release uploads both the `.tar.gz` and a `.sha256` sidecar; both
        // contain the target triple in their name. `.identifier` picks the
        // tarball deterministically instead of relying on asset order.
        .identifier(".tar.gz")
        .show_download_progress(true)
        .show_output(false)
        .current_version(current)
        .target(target)
        .no_confirm(true);
    if let Some(token) = auth_token.as_deref() {
        update_cfg.auth_token(token);
    }
    let status = update_cfg.build()?.update()?;

    writeln!(out)?;
    writeln!(out, "  updated to v{}", status.version())?;
    Ok(out)
}

/// Return true if `candidate` is a strictly newer SemVer than `current`.
/// Falls back to lexicographic comparison only if either side does not
/// parse — that keeps "v0.2.0" vs "0.2.0" prefix mismatches from
/// silently looking equal.
fn is_newer(candidate: &str, current: &str) -> bool {
    let normalize = |s: &str| s.trim_start_matches('v').to_string();
    let candidate = normalize(candidate);
    let current = normalize(current);
    match (
        semver_lite::parse(&candidate),
        semver_lite::parse(&current),
    ) {
        (Some(a), Some(b)) => a > b,
        _ => candidate.as_str() > current.as_str(),
    }
}

/// Tiny SemVer parser — enough to order `0.2.0`, `0.2.0-beta.1`, `0.10.0`.
/// We avoid pulling the full `semver` crate just for one comparison.
mod semver_lite {
    #[derive(Debug, PartialEq, Eq)]
    pub(super) struct Version {
        pub major: u64,
        pub minor: u64,
        pub patch: u64,
        /// Pre-release identifiers (e.g. `["beta", "1"]` for `-beta.1`).
        /// A version with no pre-release sorts *after* one with a
        /// pre-release — that is the SemVer rule.
        pub pre: Vec<String>,
    }

    impl PartialOrd for Version {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    impl Ord for Version {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            use std::cmp::Ordering;
            match (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
            {
                Ordering::Equal => match (self.pre.is_empty(), other.pre.is_empty()) {
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    (false, false) => self.pre.cmp(&other.pre),
                },
                ord => ord,
            }
        }
    }

    pub(super) fn parse(input: &str) -> Option<Version> {
        let (core, pre) = match input.split_once('-') {
            Some((core, pre)) => (core, pre.split('.').map(str::to_string).collect()),
            None => (input, Vec::new()),
        };
        let mut iter = core.split('.');
        let major = iter.next()?.parse().ok()?;
        let minor = iter.next()?.parse().ok()?;
        let patch = iter.next()?.parse().ok()?;
        if iter.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
            pre,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_compares_semver() {
        assert!(is_newer("0.3.0", "0.2.0-beta.1"));
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0-beta.1"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
        assert!(is_newer("v0.2.1", "v0.2.0"));
        assert!(is_newer("0.10.0", "0.2.0"));
        assert!(is_newer("0.2.0-beta.2", "0.2.0-beta.1"));
    }
}
