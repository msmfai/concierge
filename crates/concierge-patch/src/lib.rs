//! The `BSDiff` binary-patch primitive — Wabbajack's key trick for
//! non-redistributable content: a modpack ships a small `patch`, and the target
//! file is *derived* at build time from a source the user already owns
//! (`target = apply(source, patch)`). Deterministic: same source + patch always
//! yields the same bytes, so it slots into the content-addressed store and the
//! plan hash cleanly.

use qbsdiff::{Bsdiff, Bspatch};

/// Errors from patch build/apply.
#[derive(Debug)]
pub enum Error {
    /// The patch is malformed / not a `BSDiff` stream.
    Patch(String),
    /// I/O while producing the patch or target.
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Patch(m) => write!(f, "bsdiff patch: {m}"),
            Self::Io(e) => write!(f, "bsdiff io: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Derive `target` from the user-owned `source` and a `BSDiff` `patch`.
/// Deterministic — the anchor for reproducible, non-redistributable builds.
pub fn apply(source: &[u8], patch: &[u8]) -> Result<Vec<u8>> {
    let patcher = Bspatch::new(patch).map_err(|e| Error::Patch(e.to_string()))?;
    let mut out = Vec::with_capacity(usize::try_from(patcher.hint_target_size()).unwrap_or(0));
    patcher.apply(source, &mut out)?;
    Ok(out)
}

/// Produce a `BSDiff` `patch` such that `apply(source, patch) == target`. Used to
/// author patches (and to test the round-trip); the build path only needs
/// [`apply`].
pub fn diff(source: &[u8], target: &[u8]) -> Result<Vec<u8>> {
    let mut patch = Vec::new();
    Bsdiff::new(source, target)
        .compare(std::io::Cursor::new(&mut patch))
        .map_err(Error::Io)?;
    Ok(patch)
}

/// Read the whole of `r` (a tiny helper so callers can patch streams).
pub fn read_all(mut r: impl std::io::Read) -> Result<Vec<u8>> {
    let mut v = Vec::new();
    r.read_to_end(&mut v)?;
    Ok(v)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn round_trip_derives_the_exact_target_from_the_source() {
        let source = b"the quick brown fox jumps over the lazy dog, again and again";
        let target = b"the quick RED fox vaults over the lazy dog, again and AGAIN!!";
        let patch = super::diff(source, target).unwrap();
        assert!(!patch.is_empty());
        let derived = super::apply(source, &patch).unwrap();
        assert_eq!(
            derived, target,
            "apply(source, diff(source,target)) == target"
        );
    }

    #[test]
    #[allow(clippy::indexing_slicing)]
    fn apply_is_deterministic() {
        let source = vec![7u8; 4096];
        let mut target = source.clone();
        target[100..120].copy_from_slice(b"a small local change");
        let patch = super::diff(&source, &target).unwrap();
        assert_eq!(
            super::apply(&source, &patch).unwrap(),
            super::apply(&source, &patch).unwrap()
        );
        assert_eq!(super::apply(&source, &patch).unwrap(), target);
    }

    #[test]
    fn a_corrupt_patch_errors_not_panics() {
        assert!(super::apply(b"src", b"not a bsdiff stream").is_err());
    }
}
