//! GitHub Releases API: list releases and pick the asset for this platform.

use serde::Deserialize;

use crate::{Error, Result};

/// One published release (subset of the GitHub API shape; unknown fields ignored).
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// A downloadable release asset.
#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

const APP_UA: &str = "concierge-updater";

/// Fetch the recent releases for `repo` (e.g. `"msmfai/concierge"`), newest first.
/// Public endpoint — no token needed for a public repo.
pub fn fetch_releases(repo: &str) -> Result<Vec<Release>> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=30");
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(20))
        .set("User-Agent", APP_UA)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| Error::Http(e.to_string()))?;
    resp.into_json().map_err(|e| Error::Http(e.to_string()))
}

/// The substring identifying THIS platform's release asset, matching the names
/// `release.yml` produces (`…-x86_64-windows.zip`, `…-aarch64-macos.tar.gz`,
/// `…-x86_64-linux.tar.gz`).
#[must_use]
pub const fn platform_key() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// Pick the asset for `key` (see [`platform_key`]) from a release, if present.
#[must_use]
pub fn pick_asset<'a>(release: &'a Release, key: &str) -> Option<&'a Asset> {
    release.assets.iter().find(|a| a.name.contains(key))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_owned(),
            browser_download_url: format!("https://example/{name}"),
            size: 1,
        }
    }

    #[test]
    fn picks_the_matching_platform_asset() {
        let r = Release {
            tag_name: "v0.9.0".to_owned(),
            prerelease: false,
            assets: vec![
                asset("concierge-v0.9.0-x86_64-windows.zip"),
                asset("concierge-v0.9.0-aarch64-macos.tar.gz"),
                asset("concierge-v0.9.0-x86_64-linux.tar.gz"),
            ],
        };
        assert_eq!(
            pick_asset(&r, "windows").unwrap().name,
            "concierge-v0.9.0-x86_64-windows.zip"
        );
        assert_eq!(
            pick_asset(&r, "macos").unwrap().name,
            "concierge-v0.9.0-aarch64-macos.tar.gz"
        );
        assert!(pick_asset(&r, "freebsd").is_none());
    }

    #[test]
    fn deserializes_github_shape() {
        let json = r#"[{"tag_name":"v0.9.0","prerelease":false,
            "assets":[{"name":"concierge-v0.9.0-x86_64-linux.tar.gz",
            "browser_download_url":"https://x/y","size":123}]}]"#;
        let rels: Vec<Release> = serde_json::from_str(json).unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].tag_name, "v0.9.0");
        assert_eq!(rels[0].assets[0].size, 123);
    }
}
