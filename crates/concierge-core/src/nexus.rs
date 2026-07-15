//! Nexus Mods REST v1 client. Keys stay on this machine; every call is
//! user-initiated (AUP compliance).

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::repo::home;

const APP: &str = "concierge-prototype/0.1";

pub fn api_key() -> Result<String> {
    if let Ok(k) = std::env::var("NEXUS_API_KEY") {
        let k = k.trim().to_owned();
        if !k.is_empty() {
            return Ok(k);
        }
    }
    let path = home().join(".config/fo4nix/nexus-api-key");
    match std::fs::read_to_string(&path) {
        Ok(k) if !k.trim().is_empty() => Ok(k.trim().to_owned()),
        _ => Err(Error::NoApiKey),
    }
}

fn get(path: &str, key: &str) -> Result<serde_json::Value> {
    // Bound every Nexus call so a slow/down API can't hang realize or the UI.
    let resp = ureq::get(&format!("https://api.nexusmods.com{path}"))
        .timeout(std::time::Duration::from_secs(20))
        .set("apikey", key)
        .set("Application-Name", "concierge-prototype")
        .set("Application-Version", "0.1")
        .set("User-Agent", APP)
        .call()?;
    resp.into_json().map_err(|e| Error::Nexus(e.to_string()))
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub name: String,
    pub is_premium: bool,
}

pub fn validate(key: &str) -> Result<User> {
    let v = get("/v1/users/validate.json", key)?;
    serde_json::from_value(v).map_err(|e| Error::Nexus(e.to_string()))
}

/// A mod the user tracks on Nexus — the wishlist of "mods you'd have downloaded
/// anyway."
#[derive(Debug, Clone, Deserialize)]
pub struct TrackedMod {
    pub mod_id: u64,
    pub domain_name: String,
}

/// The user's tracked mods across all games (filter by `domain_name`).
pub fn tracked_mods(key: &str) -> Result<Vec<TrackedMod>> {
    let v = get("/v1/user/tracked_mods.json", key)?;
    serde_json::from_value(v).map_err(|e| Error::Nexus(e.to_string()))
}

/// A parsed `nxm://` handoff link. The `key`/`expires` token is present when the
/// link came from the site's "Mod Manager Download" button — it's the single-
/// file, time-limited authorization that lets a FREE account download that file
/// (the same token MO2/Vortex use). A bare `nxm://` link has no token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nxm {
    pub domain: String,
    pub mod_id: u64,
    pub file_id: u64,
    pub key: Option<String>,
    pub expires: Option<String>,
}

/// The nxm handoff inbox: a running Concierge polls this file and pins any
/// `nxm://` links dropped in it. Lets a browser "Mod Manager Download" (or
/// `concierge nxm <url>`) reach an already-running app without objc glue.
#[must_use]
pub fn nxm_inbox_path() -> std::path::PathBuf {
    concierge_platform::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("concierge")
        .join("nxm-inbox")
}

/// Append an nxm url to the inbox (one per line). No-op-safe on error.
pub fn append_nxm_inbox(url: &str) -> Result<()> {
    use std::io::Write as _;
    let path = nxm_inbox_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::Other(e.to_string()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| Error::Other(e.to_string()))?;
    writeln!(f, "{url}").map_err(|e| Error::Other(e.to_string()))
}

/// Read + clear the inbox, returning the valid `nxm://` links found. Pure of
/// network — the caller pins each. Removing the file is the "consume" step.
#[must_use]
pub fn drain_nxm_inbox() -> Vec<String> {
    let path = nxm_inbox_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let _ = std::fs::remove_file(&path);
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("nxm://"))
        .map(str::to_owned)
        .collect()
}

/// Parse an `nxm://<domain>/mods/<mod_id>/files/<file_id>?key=…&expires=…` link.
pub fn parse_nxm(url: &str) -> Option<Nxm> {
    let rest = url.strip_prefix("nxm://")?;
    let (path, query) = rest
        .split_once('?')
        .map_or((rest, None), |(p, q)| (p, Some(q)));
    let mut it = path.split('/');
    let domain = it.next()?.to_owned();
    if domain.is_empty() || it.next()? != "mods" {
        return None;
    }
    let mod_id = it.next()?.parse().ok()?;
    if it.next()? != "files" {
        return None;
    }
    let file_id = it.next()?.parse().ok()?;
    let (mut key, mut expires) = (None, None);
    for kv in query.into_iter().flat_map(|q| q.split('&')) {
        if let Some((k, v)) = kv.split_once('=') {
            match k {
                "key" => key = Some(v.to_owned()),
                "expires" => expires = Some(v.to_owned()),
                _ => {}
            }
        }
    }
    Some(Nxm {
        domain,
        mod_id,
        file_id,
        key,
        expires,
    })
}

#[derive(Debug, Deserialize)]
pub struct FileEntry {
    pub file_id: u64,
    pub category_name: Option<String>,
    pub version: Option<String>,
    pub file_name: String,
}

pub fn files(key: &str, domain: &str, mod_id: u64) -> Result<Vec<FileEntry>> {
    let v = get(&format!("/v1/games/{domain}/mods/{mod_id}/files.json"), key)?;
    let arr = v
        .get("files")
        .cloned()
        .ok_or_else(|| Error::Nexus("missing 'files' in response".into()))?;
    serde_json::from_value(arr).map_err(|e| Error::Nexus(e.to_string()))
}

/// Pick the file to auto-pin for a mod: prefer a `MAIN`-category file, else the
/// first current (non-old/archived) file, else the first available. This is what
/// lets a mod be resolved from just its Nexus id (no manual file-picking).
#[must_use]
pub fn pick_main(mut files: Vec<FileEntry>) -> Option<FileEntry> {
    if let Some(i) = files
        .iter()
        .position(|f| f.category_name.as_deref() == Some("MAIN"))
    {
        return Some(files.swap_remove(i));
    }
    if let Some(i) = files
        .iter()
        .position(|f| !matches!(f.category_name.as_deref(), Some("OLD_VERSION" | "ARCHIVED")))
    {
        return Some(files.swap_remove(i));
    }
    (!files.is_empty()).then(|| files.swap_remove(0))
}

/// Resolve a mod's main downloadable file from just its id — the auto-pin step.
pub fn main_file(key: &str, domain: &str, mod_id: u64) -> Result<FileEntry> {
    pick_main(files(key, domain, mod_id)?)
        .ok_or_else(|| Error::Nexus(format!("mod {mod_id} has no downloadable files")))
}

#[derive(Debug, Deserialize)]
struct DownloadLink {
    short_name: String,
    #[serde(rename = "URI")]
    uri: String,
}

/// Resolve a premium direct-download URL (percent-encoding the CDN path,
/// which contains raw spaces).
/// The human file page for a mod file — where a free (non-premium) user clicks
/// the slow download. This is the page Wabbajack opens for manual downloads; it
/// bypasses nothing (no CDN link, no key), it just deep-links the file.
#[must_use]
pub fn file_page_url(domain: &str, mod_id: u64, file_id: u64) -> String {
    format!("https://www.nexusmods.com/{domain}/mods/{mod_id}?tab=files&file_id={file_id}")
}

/// Resolve a CDN download URL for a file. With `token = Some((key, expires))`
/// (from an `nxm://` link the user minted by clicking "Mod Manager Download"),
/// the endpoint authorizes the download for a FREE account — the same path
/// MO2/Vortex use. With `token = None` it's the premium direct-download (only
/// premium accounts get a link; free accounts 403). Forges nothing.
pub fn download_url(
    key: &str,
    domain: &str,
    mod_id: u64,
    file_id: u64,
    token: Option<(&str, &str)>,
) -> Result<(String, String)> {
    let base = format!("/v1/games/{domain}/mods/{mod_id}/files/{file_id}/download_link.json");
    let path = match token {
        Some((k, e)) => format!("{base}?key={k}&expires={e}"),
        None => base,
    };
    let v = get(&path, key)?;
    let links: Vec<DownloadLink> =
        serde_json::from_value(v).map_err(|e| Error::Nexus(e.to_string()))?;
    let first = links
        .into_iter()
        .next()
        .ok_or_else(|| Error::Nexus("no download links returned".into()))?;
    Ok((encode_url(&first.uri), first.short_name))
}

fn encode_url(raw: &str) -> String {
    // Encode spaces (the only illegal char Nexus CDN URLs actually contain)
    // without disturbing existing percent-escapes or the query string.
    raw.replace(' ', "%20")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::unreadable_literal
)]
mod tests {
    use super::{append_nxm_inbox, drain_nxm_inbox, pick_main, FileEntry};

    #[test]
    fn nxm_inbox_round_trips_and_filters() {
        // append two lines (one junk) then drain -> only the nxm url, and the
        // inbox is consumed (a second drain is empty).
        let url = "nxm://skyrimspecialedition/mods/1/files/2?key=k&expires=1";
        append_nxm_inbox("not a url").unwrap();
        append_nxm_inbox(url).unwrap();
        let got = drain_nxm_inbox();
        assert!(got.contains(&url.to_owned()), "drained: {got:?}");
        assert!(!got.contains(&"not a url".to_owned()));
        assert!(drain_nxm_inbox().is_empty(), "inbox consumed");
    }

    fn f(id: u64, cat: Option<&str>, name: &str) -> FileEntry {
        FileEntry {
            file_id: id,
            category_name: cat.map(str::to_owned),
            version: None,
            file_name: name.to_owned(),
        }
    }

    #[test]
    fn pick_main_prefers_main_then_current_then_first() {
        // MAIN wins even if listed after others
        let got = pick_main(vec![
            f(1, Some("OPTIONAL"), "opt"),
            f(2, Some("MAIN"), "main"),
            f(3, Some("OLD_VERSION"), "old"),
        ])
        .unwrap();
        assert_eq!(got.file_id, 2);
        // no MAIN → first non-old/archived
        let got = pick_main(vec![
            f(9, Some("OLD_VERSION"), "old"),
            f(5, Some("OPTIONAL"), "opt"),
        ])
        .unwrap();
        assert_eq!(got.file_id, 5);
        // only old → still returns something
        let got = pick_main(vec![f(7, Some("OLD_VERSION"), "old")]).unwrap();
        assert_eq!(got.file_id, 7);
        // empty → none
        assert!(pick_main(vec![]).is_none());
    }

    #[test]
    fn parse_nxm_extracts_ids_and_token() {
        use super::parse_nxm;
        let n = parse_nxm("nxm://skyrimspecialedition/mods/12604/files/749043?key=abc&expires=99")
            .unwrap();
        assert_eq!(n.domain, "skyrimspecialedition");
        assert_eq!(n.mod_id, 12604);
        assert_eq!(n.file_id, 749043);
        // the free-user download token is captured
        assert_eq!(n.key.as_deref(), Some("abc"));
        assert_eq!(n.expires.as_deref(), Some("99"));
        // a bare link (no click token) parses but has no token
        let bare = parse_nxm("nxm://fallout4/mods/1/files/2").unwrap();
        assert!(bare.key.is_none() && bare.expires.is_none());
        // rejects malformed
        assert!(parse_nxm("https://nexusmods.com/x").is_none());
        assert!(parse_nxm("nxm://domain/mods/notanumber/files/2").is_none());
        assert!(parse_nxm("nxm://domain/collections/1/files/2").is_none());
    }
}
