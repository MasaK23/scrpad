//! "Check for updates…": asks GitHub for the latest release, only when the user chooses to.
//! Nothing here runs on its own; there is no background or start-up check.

use std::time::Duration;

use serde::Deserialize;

const LATEST_RELEASE: &str = "https://api.github.com/repos/mhavo/scrpad/releases/latest";
/// Only links into this repository's releases are opened from the answer.
const RELEASES_PAGE: &str = "https://github.com/mhavo/scrpad/releases/";

#[derive(Debug, PartialEq)]
pub enum Outcome {
    UpToDate { current: String },
    Available { version: String, url: String },
    Failed(String),
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
}

/// One blocking HTTPS request; call it from a worker thread.
pub fn check() -> Outcome {
    let current = env!("CARGO_PKG_VERSION");
    match fetch_latest() {
        Ok(release) => evaluate(current, &release.tag_name, &release.html_url),
        Err(e) => Outcome::Failed(e),
    }
}

fn fetch_latest() -> Result<Release, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(concat!("scrpad/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut response = agent.get(LATEST_RELEASE).header("Accept", "application/vnd.github+json").call().map_err(|e| e.to_string())?;
    let body = response.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| format!("unexpected answer from GitHub: {e}"))
}

/// Compares `current` with the release tag (`v1.2.3`).
fn evaluate(current: &str, tag: &str, html_url: &str) -> Outcome {
    let parse = |v: &str| semver::Version::parse(v.trim_start_matches('v'));
    let (Ok(current_version), Ok(latest)) = (parse(current), parse(tag)) else {
        return Outcome::Failed(format!("unexpected version “{tag}”"));
    };
    if latest <= current_version {
        return Outcome::UpToDate { current: current.to_owned() };
    }
    let url = if html_url.starts_with(RELEASES_PAGE) { html_url.to_owned() } else { format!("{RELEASES_PAGE}latest") };
    Outcome::Available { version: latest.to_string(), url }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://github.com/mhavo/scrpad/releases/tag/v0.2.0";

    #[test]
    fn newer_release_is_available() {
        assert_eq!(evaluate("0.1.2", "v0.2.0", PAGE), Outcome::Available { version: "0.2.0".into(), url: PAGE.into() });
        assert_eq!(evaluate("0.1.2", "v0.1.10", PAGE), Outcome::Available { version: "0.1.10".into(), url: PAGE.into() });
    }

    #[test]
    fn same_or_older_release_is_up_to_date() {
        assert_eq!(evaluate("0.2.0", "v0.2.0", PAGE), Outcome::UpToDate { current: "0.2.0".into() });
        assert_eq!(evaluate("0.2.0", "v0.1.9", PAGE), Outcome::UpToDate { current: "0.2.0".into() });
    }

    #[test]
    fn foreign_links_are_never_opened() {
        let Outcome::Available { url, .. } = evaluate("0.1.0", "v0.2.0", "https://evil.example/scrpad") else { panic!() };
        assert_eq!(url, "https://github.com/mhavo/scrpad/releases/latest");
    }

    #[test]
    fn garbage_tags_fail() {
        assert!(matches!(evaluate("0.1.0", "nightly", PAGE), Outcome::Failed(_)));
    }

    /// Talks to GitHub: run by hand with `cargo test live_check -- --ignored`.
    #[test]
    #[ignore = "network"]
    fn live_check() {
        let outcome = check();
        assert!(!matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    }
}
