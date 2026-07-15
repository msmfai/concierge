//! Nix-language front-end: evaluate a `.nix` modpack expression into the very
//! same [`Manifest`](crate::manifest::Manifest) the TOML tier produces.
//!
//! We borrow the Nix **language** (pure evaluation → JSON), not the build system
//! or the store — so a modpack works cross-platform (the evaluation is pure and
//! the acquisition stays Concierge's own hash-pinned fetchers). Because both
//! front-ends deserialize into the identical `Manifest`, the plan and its hash
//! are front-end-independent: `nix eval` and `toml parse` of the same modpack
//! MUST produce a byte-identical plan. That equality is the differential oracle.
//!
//! The real-`nix` build system (fixed-output derivations, `/nix/store`) is an
//! optional accelerator on platforms that have it — never a baseline dependency.

use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};
use crate::manifest::Manifest;

/// Is a Nix evaluator available on this machine? (The Nix *language* tier; the
/// TOML tier needs nothing.)
#[must_use]
pub fn available() -> bool {
    Command::new("nix")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Evaluate a `.nix` modpack file to JSON via `nix eval`, then deserialize +
/// validate it as a [`Manifest`]. Pure evaluation — no store, no build.
pub fn eval_manifest(config_file: &Path) -> Result<Manifest> {
    let out = Command::new("nix")
        .args(["eval", "--json", "--file"])
        .arg(config_file)
        .output()
        .map_err(|e| Error::Other(format!("nix eval: {e} (is Nix installed?)")))?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "nix eval failed for {}:\n{}",
            config_file.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let json = String::from_utf8_lossy(&out.stdout);
    Manifest::from_json(&json)
}

/// `nix build` a fixed-output fetcher expression (fetchGit/fetchurl/…) into
/// `/nix/store` — Nix verifies it against its SRI/rev — and return the store
/// out-path. This is the optional Nix-FOD acquisition tier.
pub fn build_fod(expr: &str) -> Result<std::path::PathBuf> {
    // Source fetchers (builtins.fetchGit/fetchTarball/fetchurl) realize during
    // EVALUATION and coerce to their `/nix/store` path — Nix verifies the fetch
    // against its rev/SRI. `toString` triggers that realization and yields the
    // path. (For real derivation fetchers, wrap in a derivation yourself.)
    let out = Command::new("nix")
        .args(["eval", "--impure", "--raw", "--expr"])
        .arg(format!("toString ({expr})"))
        .output()
        .map_err(|e| Error::Other(format!("nix eval: {e} (is Nix installed?)")))?;
    if !out.status.success() {
        return Err(Error::Other(format!(
            "nix FOD build failed:\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let path = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    if !path.exists() {
        return Err(Error::Other(format!(
            "nix FOD produced no store path (got {:?})",
            path.display()
        )));
    }
    Ok(path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn build_fod_realizes_a_source_into_the_store() {
        if !available() {
            eprintln!("nix not present; skipping");
            return;
        }
        // a local path FOD (no network): builtins.path imports it into the store
        let src = std::env::temp_dir().join(format!("cc-fod-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("marker.txt"), b"concierge-fod").unwrap();
        let expr = format!(
            "builtins.path {{ path = \"{}\"; name = \"src\"; }}",
            src.display()
        );
        let out = build_fod(&expr).expect("nix realizes the FOD");
        assert!(out.exists(), "store path exists");
        assert_eq!(
            std::fs::read_to_string(out.join("marker.txt")).unwrap(),
            "concierge-fod",
            "the FOD carries our content"
        );
        let _ = std::fs::remove_dir_all(&src);
    }
}
