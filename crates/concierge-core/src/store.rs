//! The fetch phase: fixed-output acquisition into the content-addressed store.
//!
//! Premium Nexus keys make this fully automatic; otherwise we ingest from the
//! browser-download inbox (requireFile semantics). TOFU: unpinned archives are
//! hashed and the pin printed for the user to commit into the manifest.

use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use crate::error::{Error, IoCtx, Result};
use crate::nexus;
use crate::plan::{Plan, PlannedMod, Source};
use crate::repo::{inbox_dir, md5_file, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    /// Already in the store.
    Present(PathBuf),
    /// Downloaded/ingested and stored.
    Stored(PathBuf),
    /// Stored, but the manifest has no pin yet — user must commit this hash.
    NeedsPin { path: PathBuf, md5: String },
    /// Nothing we can do without the user (inbox miss / no key).
    Blocked { instructions: String },
}

pub fn fetch_all(repo: &Repo, plan: &Plan) -> Result<Vec<(String, FetchOutcome)>> {
    std::fs::create_dir_all(repo.store()).ctx(&repo.store())?;
    // One `whoami` for the whole batch: the premium API download only works for
    // premium accounts (it 403s for free ones), so free users take the
    // click/manual path instead of hard-failing.
    let premium = nexus_premium();
    plan.mods
        .iter()
        .map(|m| {
            Ok((
                m.name.clone(),
                fetch_one(repo, plan.game.nexus_domain.as_deref(), m, premium)?,
            ))
        })
        .collect()
}

/// True only if a Nexus API key is set AND the account is premium. No key, a bad
/// key, a free account, or a timed-out/failed `whoami` all return `false` — the
/// free (click/manual) path. Cached for a few minutes so we don't hit the
/// network (a bounded call) on every realize.
fn nexus_premium() -> bool {
    use std::sync::Mutex;
    use std::time::Instant;
    static CACHE: Mutex<Option<(bool, Instant)>> = Mutex::new(None);
    const TTL_SECS: u64 = 300;

    if let Ok(guard) = CACHE.lock() {
        if let Some((val, at)) = *guard {
            if at.elapsed().as_secs() < TTL_SECS {
                return val;
            }
        }
    }
    let val = nexus::api_key()
        .ok()
        .and_then(|k| nexus::validate(&k).ok())
        .is_some_and(|u| u.is_premium);
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((val, Instant::now()));
    }
    val
}

/// Build a `nix = …` fixed-output derivation into the store — the opt-in tier.
/// Without the `nix-source` feature this always errors (the `nix` binary is
/// never invoked, nor even compiled in).
fn fetch_nix_fod(repo: &Repo, name: &str, file: &str, expr: &str) -> Result<PathBuf> {
    #[cfg(not(feature = "nix-source"))]
    {
        let _ = (repo, name, file, expr);
        Err(Error::Other(
            "this mod has a `nix = …` source; rebuild Concierge with \
             `--features nix-source` (and Nix installed) to use it"
                .to_owned(),
        ))
    }
    #[cfg(feature = "nix-source")]
    {
        // nix build the FOD (Nix verifies it against its SRI/rev) -> store path
        let out = crate::nix::build_fod(expr)?;
        eprintln!("  nix-fod   {name}: built {}", out.display());
        let tmp = repo.store().join(format!(".tmp-{file}"));
        if out.is_dir() {
            crate::pipeline::deterministic_tar(&out, &tmp)?;
        } else {
            std::fs::copy(&out, &tmp).ctx(&tmp)?;
        }
        Ok(tmp)
    }
}

fn fetch_one(
    repo: &Repo,
    domain: Option<&str>,
    m: &PlannedMod,
    premium: bool,
) -> Result<FetchOutcome> {
    if let Some(md5) = &m.md5 {
        let sp = repo.store_path(md5, &m.file);
        if sp.exists() {
            return Ok(FetchOutcome::Present(sp));
        }
    }

    let staged: Option<PathBuf> = match &m.source {
        Source::Url { url } => Some(download(repo, &m.file, url)?),
        Source::Pipeline { steps } => {
            // impure `run` is honored only for hand-written manifests; here the
            // whole plan came from the user's manifest, so allow it. (The AI
            // path validates separately with allow_run = false.)
            let work = repo.store().join(format!(".pipeline-{}", m.file));
            let _ = std::fs::remove_dir_all(&work);
            let out = crate::pipeline::run(steps, &work, &m.file, true)?;
            let tmp = repo.store().join(format!(".tmp-{}", m.file));
            std::fs::rename(&out, &tmp)
                .or_else(|_| std::fs::copy(&out, &tmp).map(|_| ()))
                .ctx(&tmp)?;
            let _ = std::fs::remove_dir_all(&work);
            Some(tmp)
        }
        Source::Nix { expr } => Some(fetch_nix_fod(repo, &m.name, &m.file, expr)?),
        // Premium accounts get the automatic API download. Free accounts (or on
        // ANY API error — 403, expired, rate-limited) fall through to the
        // click/manual path below rather than hard-failing the whole realize.
        Source::Nexus { mod_id, file_id } => {
            let api = premium
                .then(|| nexus::api_key().ok())
                .flatten()
                .zip(domain)
                .and_then(|(key, d)| nexus::download_url(&key, d, *mod_id, *file_id, None).ok());
            match api {
                Some((url, server)) => {
                    eprintln!(
                        "  nexus     {}: resolved via premium API ({server})",
                        m.name
                    );
                    Some(download(repo, &m.file, &url)?)
                }
                None => None,
            }
        }
        Source::Inbox => None,
    };

    // Wabbajack-style manual ingest: find the archive in ~/Downloads by name OR
    // by content hash (md5 pin and/or Wabbajack xxHash64 key).
    let staged = if let Some(p) = staged {
        p
    } else if let Some(found) = find_in_inbox(&m.file, m.md5.as_deref(), m.xxhash.as_deref())? {
        let tmp = repo.store().join(format!(".tmp-{}", m.file));
        std::fs::copy(&found, &tmp).ctx(&found)?;
        tmp
    } else {
        // Nothing yet. For a Nexus source, open the exact file page in the
        // browser — the same manual path Wabbajack uses for free users. We
        // resolve/forge no CDN link; the user clicks the slow download.
        let (page, opened) = match &m.source {
            Source::Nexus { mod_id, file_id } => match domain {
                Some(d) => {
                    let url = nexus::file_page_url(d, *mod_id, *file_id);
                    open_browser(&url);
                    (url, true)
                }
                None => ("(no Nexus domain for this game)".to_owned(), false),
            },
            _ => (
                "(no automatic source — supply the archive)".to_owned(),
                false,
            ),
        };
        return Ok(FetchOutcome::Blocked {
            instructions: format!(
                "'{}' isn't cached. Free-user options (both TOS-compliant): click \
                 'Mod Manager Download' on the file page (one click, auto-downloads), or \
                 save the file to ~/Downloads — then re-run.{}\n            {page}",
                m.file,
                if opened {
                    " (opened the file page for you)"
                } else {
                    ""
                }
            ),
        });
    };

    let got = md5_file(&staged)?;
    if let Some(expected) = &m.md5 {
        if *expected != got {
            std::fs::remove_file(&staged).ctx(&staged)?;
            return Err(Error::HashMismatch {
                name: m.name.clone(),
                expected: expected.clone(),
                got,
            });
        }
    }
    let dest = repo.store_path(&got, &m.file);
    std::fs::rename(&staged, &dest).ctx(&dest)?;
    if m.md5.is_none() {
        return Ok(FetchOutcome::NeedsPin {
            path: dest,
            md5: got,
        });
    }
    Ok(FetchOutcome::Stored(dest))
}

/// Download one Nexus file using an `nxm://` token — the free-user, one-click,
/// TOS-sanctioned authorization the user minted by clicking "Mod Manager
/// Download" — and store it by md5. Returns `(md5, filename)` so the caller can
/// pin the `[[mod]]`. Forges nothing: the token is the user's own.
pub fn acquire_nxm(
    repo: &Repo,
    domain: &str,
    mod_id: u64,
    file_id: u64,
    api_key: &str,
    nxm_key: &str,
    expires: &str,
) -> Result<(String, String)> {
    std::fs::create_dir_all(repo.store()).ctx(&repo.store())?;
    let (url, _server) =
        nexus::download_url(api_key, domain, mod_id, file_id, Some((nxm_key, expires)))?;
    let file = url
        .split('?')
        .next()
        .and_then(|u| u.rsplit('/').next())
        .map(|f| f.replace("%20", " "))
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| format!("nexus-{mod_id}-{file_id}.archive"));
    let staged = download(repo, &file, &url)?;
    let md5 = md5_file(&staged)?;
    let dest = repo.store_path(&md5, &file);
    std::fs::rename(&staged, &dest).ctx(&dest)?;
    Ok((md5, file))
}

/// Wabbajack's hash-detect ingest: scan the download inbox for a file named
/// `name`, or (failing that) any file whose md5 or xxHash64 matches a pin. This
/// is why a manually-downloaded archive installs even if the browser renamed it.
fn find_in_inbox(
    name: &str,
    md5_pin: Option<&str>,
    xx_pin: Option<&str>,
) -> Result<Option<PathBuf>> {
    find_in_dir(&inbox_dir(), name, md5_pin, xx_pin)
}

fn find_in_dir(
    dir: &std::path::Path,
    name: &str,
    md5_pin: Option<&str>,
    xx_pin: Option<&str>,
) -> Result<Option<PathBuf>> {
    let exact = dir.join(name);
    if exact.is_file() {
        return Ok(Some(exact));
    }
    if md5_pin.is_none() && xx_pin.is_none() || !dir.is_dir() {
        return Ok(None);
    }
    for entry in std::fs::read_dir(dir).ctx(dir)?.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if let Some(want) = md5_pin {
            if md5_file(&p)? == want {
                return Ok(Some(p));
            }
        }
        if let Some(want) = xx_pin {
            let bytes = std::fs::read(&p).ctx(&p)?;
            if concierge_hash::matches_xxhash64_base64(&bytes, want) {
                return Ok(Some(p));
            }
        }
    }
    Ok(None)
}

/// Best-effort: open a URL in the user's browser (the manual-download page).
fn open_browser(url: &str) {
    // Suppress the launch under tests / headless runs.
    if std::env::var_os("CONCIERGE_NO_BROWSER").is_some() {
        return;
    }
    let _ = concierge_platform::open_url(url);
}

// --- verified multi-host fetch (P0: makes an imported list actionable) ---

/// A neutral remote descriptor the verified fetcher dispatches on (importers
/// map their own source enums into this).
#[derive(Debug, Clone)]
pub enum Remote {
    Nexus {
        game_domain: String,
        mod_id: u64,
        file_id: u64,
    },
    Http {
        url: String,
    },
    /// A source we recognize but don't fetch (MEGA/GoogleDrive/CDN/…): reported,
    /// never silently skipped.
    Unsupported {
        kind: String,
    },
}

/// The outcome of a hash-verified fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifiedFetch {
    /// Already in the store and its xxHash64 matched — no download.
    Cached(PathBuf),
    /// Downloaded and its xxHash64 matched the expected value.
    Fetched(PathBuf),
    /// Downloaded, but the content hash did not match (bad file discarded).
    HashMismatch { expected: String, actual: String },
    /// A Nexus source but no premium key is configured.
    NoKey,
    /// A source we don't fetch (reported).
    Unsupported { kind: String },
}

/// Fetch `name` from `remote` into `store_dir`, verifying its xxHash64 equals
/// `expected_b64` (Wabbajack's base64 digest). A matching copy already present
/// is a cache hit (no download); a mismatch discards the download.
pub fn fetch_verified(
    store_dir: &std::path::Path,
    name: &str,
    remote: &Remote,
    expected_b64: &str,
) -> Result<VerifiedFetch> {
    std::fs::create_dir_all(store_dir).ctx(store_dir)?;
    let dest = store_dir.join(name);
    if dest.is_file() {
        let bytes = std::fs::read(&dest).ctx(&dest)?;
        if concierge_hash::matches_xxhash64_base64(&bytes, expected_b64) {
            return Ok(VerifiedFetch::Cached(dest));
        }
    }

    let url = match remote {
        Remote::Http { url } => url.clone(),
        Remote::Unsupported { kind } => {
            return Ok(VerifiedFetch::Unsupported { kind: kind.clone() })
        }
        Remote::Nexus {
            game_domain,
            mod_id,
            file_id,
        } => {
            let Ok(key) = nexus::api_key() else {
                return Ok(VerifiedFetch::NoKey);
            };
            let (url, _server) = nexus::download_url(&key, game_domain, *mod_id, *file_id, None)?;
            url
        }
    };

    let tmp = store_dir.join(format!(".tmp-verify-{name}"));
    eprintln!(
        "  fetching  {name} <- {}",
        url.split('?').next().unwrap_or(&url)
    );
    let resp = ureq::get(&url)
        .set("User-Agent", "concierge-prototype/0.1")
        .call()?;
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf).ctx(&tmp)?;

    let actual = concierge_hash::xxhash64_base64(&buf);
    if actual != expected_b64.trim() {
        return Ok(VerifiedFetch::HashMismatch {
            expected: expected_b64.trim().to_owned(),
            actual,
        });
    }
    std::fs::write(&dest, &buf).ctx(&dest)?;
    Ok(VerifiedFetch::Fetched(dest))
}

fn download(repo: &Repo, file: &str, url: &str) -> Result<PathBuf> {
    let tmp = repo.store().join(format!(".tmp-{file}"));
    eprintln!(
        "  fetching  {file} <- {}",
        url.split('?').next().unwrap_or(url)
    );
    if let Some(local) = url.strip_prefix("file://") {
        std::fs::copy(local, &tmp).ctx(&tmp)?;
        return Ok(tmp);
    }
    let resp = ureq::get(url)
        .set("User-Agent", "concierge-prototype/0.1")
        .call()?;
    let mut out = std::fs::File::create(&tmp).ctx(&tmp)?;
    std::io::copy(&mut resp.into_reader(), &mut out).ctx(&tmp)?;
    out.flush().ctx(&tmp)?;
    Ok(tmp)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod inbox_tests {
    use super::find_in_dir;

    #[test]
    fn detects_a_renamed_download_by_xxhash_and_md5() {
        let dir = std::env::temp_dir().join(format!("cc-inbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = b"a manually downloaded archive body";
        // the browser saved it under a different name than the manifest expects
        std::fs::write(dir.join("Some Mod-1234-1-0.7z"), bytes).unwrap();

        let want_md5 = hex::encode(<md5::Md5 as md5::Digest>::digest(bytes));
        let want_xx = concierge_hash::xxhash64_base64(bytes);

        // by xxHash64 (Wabbajack's key) — different name, still found
        let hit = find_in_dir(&dir, "expected-name.7z", None, Some(&want_xx)).unwrap();
        assert_eq!(
            hit.as_deref(),
            Some(dir.join("Some Mod-1234-1-0.7z").as_path())
        );
        // by md5 (Concierge's pin)
        let hit = find_in_dir(&dir, "expected-name.7z", Some(&want_md5), None).unwrap();
        assert!(hit.is_some());
        // no pins + no name match -> nothing (no false positive)
        assert!(find_in_dir(&dir, "expected-name.7z", None, None)
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod free_user_tests {
    use super::{fetch_one, FetchOutcome};
    use crate::plan::{PlannedMod, Source};
    use crate::repo::Repo;

    #[test]
    fn free_user_nexus_fetch_blocks_instead_of_erroring() {
        // A non-premium user (premium=false) with an uncached Nexus mod must reach
        // a graceful Blocked (click/manual handoff), NEVER a hard error.
        std::env::set_var("CONCIERGE_NO_BROWSER", "1");
        let tmp = std::env::temp_dir().join(format!("cg-free-{}", std::process::id()));
        let repo = Repo::at(&tmp.join("games").join("g").join("profiles").join("p"));
        let m = PlannedMod {
            name: "TestMod".to_owned(),
            version: "1".to_owned(),
            source: Source::Nexus {
                mod_id: 1,
                file_id: 2,
            },
            // a name that won't exist in ~/Downloads, and no md5/xxhash pins,
            // so the inbox scan finds nothing and we reach Blocked.
            file: format!("cg-uniq-{}-absent.7z", std::process::id()),
            md5: None,
            xxhash: None,
            install_root: "data".to_owned(),
            subdir: None,
            fomod: None,
            exclude: Vec::new(),
            plugins: Vec::new(),
            patch: None,
        };
        let out = fetch_one(&repo, Some("skyrimspecialedition"), &m, false).unwrap();
        assert!(
            matches!(out, FetchOutcome::Blocked { .. }),
            "free user must Block, not error: {out:?}"
        );
    }
}
