//! Shared helpers for the live suites (`live_games`, `live_mutate`): repo
//! discovery, real-install enumeration, running the actual binary, and the
//! pristine fingerprint that proves an install came out untouched.

#![allow(dead_code)] // each test crate uses a subset

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Repo root (the git workspace), from this crate's manifest dir.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/concierge sits two levels under the workspace")
        .to_path_buf()
}

/// `games/<g>` dirs that have at least one profile, honoring the
/// `CONCIERGE_LIVE_GAMES` filter.
pub fn game_dirs() -> Vec<PathBuf> {
    let filter: Option<Vec<String>> = std::env::var("CONCIERGE_LIVE_GAMES")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_owned()).collect());
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(repo_root().join("games")) else {
        return out;
    };
    for e in entries.flatten() {
        let dir = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(f) = &filter {
            if !f.contains(&name) {
                continue;
            }
        }
        if dir.join("profiles").is_dir() {
            out.push(dir);
        }
    }
    out.sort();
    out
}

pub fn profiles(game_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(game_dir.join("profiles")) {
        for e in entries.flatten() {
            if e.path().join("manifest.toml").exists() {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

/// The game's pristine path, read from its first profile's manifest (all
/// profiles of a game share the pristine install).
pub fn pristine_of(game_dir: &Path) -> Option<PathBuf> {
    concierge_games::register();
    let profile = profiles(game_dir).into_iter().next()?;
    let m = concierge::manifest::Manifest::load(&profile).ok()?;
    Some(m.game.pristine)
}

/// The game's declared kind, from its first profile's manifest.
pub fn kind_of(game_dir: &Path) -> Option<String> {
    concierge_games::register();
    let profile = profiles(game_dir).into_iter().next()?;
    let m = concierge::manifest::Manifest::load(&profile).ok()?;
    Some(m.game.kind)
}

/// Run the real binary against a profile; returns (success, stdout+stderr).
pub fn concierge(profile: &Path, args: &[&str], stdin: Option<&str>) -> (bool, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_concierge"));
    cmd.args(args).env("CONCIERGE_REPO", profile);
    let out = if let Some(input) = stdin {
        use std::io::Write as _;
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("spawn concierge");
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(input.as_bytes())
            .expect("write step script");
        child.wait_with_output().expect("wait for concierge")
    } else {
        cmd.output().expect("run concierge")
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// Every regular file under `root` → (size, mtime). `.DS_Store` excluded to
/// match the inventory's view. This is the untouched-proof fingerprint.
pub fn snapshot(root: &Path) -> BTreeMap<String, (u64, SystemTime)> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, (u64, SystemTime)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            if e.file_name().to_string_lossy() == ".DS_Store" {
                continue;
            }
            let p = e.path();
            let Ok(meta) = std::fs::metadata(&p) else {
                continue;
            };
            if meta.is_dir() {
                walk(root, &p, out);
            } else if meta.is_file() {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                out.insert(rel, (meta.len(), meta.modified().unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Human-readable diff of two snapshots (first 20 changed/added/removed rels).
pub fn snapshot_diff(
    before: &BTreeMap<String, (u64, SystemTime)>,
    after: &BTreeMap<String, (u64, SystemTime)>,
) -> Vec<String> {
    after
        .iter()
        .filter(|(k, v)| before.get(*k) != Some(v))
        .map(|(k, _)| k.clone())
        .chain(before.keys().filter(|k| !after.contains_key(*k)).cloned())
        .take(20)
        .collect()
}
