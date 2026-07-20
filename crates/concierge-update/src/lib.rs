//! In-app auto-updater. Checks GitHub Releases for a newer Concierge, downloads
//! the platform asset, verifies it, and swaps it in on relaunch — retiring the
//! hand-delivery (Bear Share) loop.
//!
//! The crate is split so the *decision* logic (which release, is it newer) is
//! pure and exhaustively tested; the network fetch, download+verify, and the
//! platform swap are thin IO layers over it.

mod apply;
mod github;

pub use apply::{apply_staged, cleanup_old, relaunch, stage_update, StagedUpdate};
pub use github::{fetch_releases, pick_asset, platform_key, Asset, Release};

use std::cmp::Ordering;

/// Errors from the update flow.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("network/API error: {0}")]
    Http(String),
    #[error("no release asset matched this platform ({0})")]
    NoAsset(String),
    #[error("download verification failed: {0}")]
    Verify(String),
    #[error("io error: {0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Which releases the user opts into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Only full releases (GitHub `prerelease == false`).
    Stable,
    /// Full releases AND pre-releases.
    Beta,
}

impl Channel {
    /// Does this channel accept a release with the given prerelease flag?
    #[must_use]
    pub const fn accepts(self, prerelease: bool) -> bool {
        match self {
            Self::Stable => !prerelease,
            Self::Beta => true,
        }
    }
}

/// A parsed semantic version: `MAJOR.MINOR.PATCH` with an optional pre-release
/// tail (`-beta.1`). Ordering is semver-shaped: a full release outranks any
/// pre-release of the same core, and two pre-releases compare lexically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// `None` for a full release; `Some("beta.1")` for a pre-release.
    pub pre: Option<String>,
}

impl Version {
    /// Parse a tag like `v0.9.0`, `0.9.0`, or `v0.9.0-beta.1`. A leading `v` is
    /// optional. Returns `None` if the core isn't three dot-separated integers.
    #[must_use]
    pub fn parse(tag: &str) -> Option<Self> {
        let s = tag.strip_prefix('v').unwrap_or(tag);
        let (core, pre) = s
            .split_once('-')
            .map_or((s, None), |(c, p)| (c, Some(p.to_owned())));
        let mut it = core.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next()?.parse().ok()?;
        let patch = it.next()?.parse().ok()?;
        if it.next().is_some() {
            return None; // more than three components
        }
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    #[must_use]
    pub fn display(&self) -> String {
        self.pre.as_ref().map_or_else(
            || format!("{}.{}.{}", self.major, self.minor, self.patch),
            |p| format!("{}.{}.{}-{p}", self.major, self.minor, self.patch),
        )
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                // A full release outranks a pre-release of the same core.
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

/// The outcome of a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Already on the newest release the channel offers.
    UpToDate,
    /// A newer release is available.
    Update { tag: String, version: Version },
}

/// Pure decision: given the running version, a channel, and the releases seen on
/// GitHub, decide whether to update and to what. Picks the highest channel-eligible
/// version; returns `UpToDate` if nothing beats `current`.
#[must_use]
pub fn select_update(current: &Version, channel: Channel, releases: &[Release]) -> Decision {
    let best = releases
        .iter()
        .filter(|r| channel.accepts(r.prerelease))
        .filter_map(|r| Version::parse(&r.tag_name).map(|v| (v, &r.tag_name)))
        .max_by(|(a, _), (b, _)| a.cmp(b));
    match best {
        Some((v, tag)) if v > *current => Decision::Update {
            tag: tag.clone(),
            version: v,
        },
        _ => Decision::UpToDate,
    }
}

/// The current build's version, parsed from `CARGO_PKG_VERSION` of the CALLER.
/// Callers pass their own `env!("CARGO_PKG_VERSION")` so the version reflects the
/// binary, not this crate.
#[must_use]
pub fn parse_current(pkg_version: &str) -> Version {
    Version::parse(pkg_version).unwrap_or(Version {
        major: 0,
        minor: 0,
        patch: 0,
        pre: None,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn rel(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.to_owned(),
            prerelease,
            assets: Vec::new(),
        }
    }

    #[test]
    fn parse_roundtrips() {
        for t in ["0.9.0", "1.2.3", "0.8.9", "2.0.0-beta.1", "0.9.0-rc.2"] {
            let v = Version::parse(t).unwrap();
            assert_eq!(v.display(), t, "roundtrip {t}");
            // leading v is accepted and normalised away
            assert_eq!(Version::parse(&format!("v{t}")).unwrap(), v);
        }
    }

    #[test]
    fn parse_rejects_malformed() {
        for bad in ["", "v", "1", "1.2", "1.2.3.4", "a.b.c", "1.x.0"] {
            assert!(Version::parse(bad).is_none(), "should reject {bad:?}");
        }
    }

    #[test]
    fn ordering_is_semver_shaped() {
        let v = Version::parse;
        assert!(v("0.9.0").unwrap() > v("0.8.9").unwrap());
        assert!(v("0.8.10").unwrap() > v("0.8.9").unwrap());
        assert!(v("1.0.0").unwrap() > v("0.99.99").unwrap());
        // a full release outranks its own pre-releases
        assert!(v("1.0.0").unwrap() > v("1.0.0-beta.1").unwrap());
        // a later pre-release outranks an earlier one
        assert!(v("1.0.0-beta.2").unwrap() > v("1.0.0-beta.1").unwrap());
        // a higher core beats a lower core regardless of pre
        assert!(v("1.1.0-beta.1").unwrap() > v("1.0.0").unwrap());
    }

    #[test]
    fn ordering_is_total_and_reflexive() {
        let all = [
            "0.8.9",
            "0.9.0",
            "0.9.1",
            "1.0.0-beta.1",
            "1.0.0-beta.2",
            "1.0.0",
            "1.0.1",
        ]
        .map(|t| Version::parse(t).unwrap());
        for a in &all {
            assert_eq!(a.cmp(a), Ordering::Equal);
            for b in &all {
                assert_eq!(a.cmp(b), b.cmp(a).reverse(), "antisymmetry {a:?} {b:?}");
            }
        }
    }

    #[test]
    fn stable_channel_ignores_prereleases() {
        let cur = Version::parse("0.8.9").unwrap();
        let releases = [rel("v1.0.0-beta.1", true), rel("v0.9.0", false)];
        // stable picks the newest NON-prerelease
        assert_eq!(
            select_update(&cur, Channel::Stable, &releases),
            Decision::Update {
                tag: "v0.9.0".to_owned(),
                version: Version::parse("0.9.0").unwrap()
            }
        );
        // beta picks the beta because it outranks 0.9.0
        assert_eq!(
            select_update(&cur, Channel::Beta, &releases),
            Decision::Update {
                tag: "v1.0.0-beta.1".to_owned(),
                version: Version::parse("1.0.0-beta.1").unwrap()
            }
        );
    }

    #[test]
    fn up_to_date_when_current_is_highest() {
        let cur = Version::parse("1.0.0").unwrap();
        let releases = [
            rel("v1.0.0", false),
            rel("v0.9.0", false),
            rel("v1.0.0-beta.9", true),
        ];
        assert_eq!(
            select_update(&cur, Channel::Stable, &releases),
            Decision::UpToDate
        );
        // beta: the 1.0.0-beta.9 is LOWER than the 1.0.0 release, so still up to date
        assert_eq!(
            select_update(&cur, Channel::Beta, &releases),
            Decision::UpToDate
        );
    }

    #[test]
    fn never_downgrades() {
        // With only older releases available, never proposes an update.
        let cur = Version::parse("2.0.0").unwrap();
        let releases = [rel("v1.9.9", false), rel("v2.0.0-beta.1", true)];
        assert_eq!(
            select_update(&cur, Channel::Beta, &releases),
            Decision::UpToDate
        );
    }

    #[test]
    fn empty_releases_is_up_to_date() {
        let cur = Version::parse("0.8.9").unwrap();
        assert_eq!(
            select_update(&cur, Channel::Stable, &[]),
            Decision::UpToDate
        );
    }

    #[test]
    fn ignores_unparseable_tags() {
        let cur = Version::parse("0.8.9").unwrap();
        let releases = [
            rel("nightly", false),
            rel("latest", false),
            rel("v0.9.0", false),
        ];
        assert_eq!(
            select_update(&cur, Channel::Stable, &releases),
            Decision::Update {
                tag: "v0.9.0".to_owned(),
                version: Version::parse("0.9.0").unwrap()
            }
        );
    }
}
