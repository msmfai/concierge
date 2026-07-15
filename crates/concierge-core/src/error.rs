use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no Concierge profile found from {0}. Open the Concierge app and use \"+ add game\", or run `concierge init <game>` in a workspace, or set CONCIERGE_REPO to a profile directory.")]
    RepoNotFound(PathBuf),
    #[error("io: {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("manifest: {0}")]
    Manifest(String),
    #[error("manifest parse: {0}")]
    ManifestParse(#[from] toml::de::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("mod '{name}': archive not pinned (md5 empty) — run `concierge fetch` and pin the printed hash")]
    Unpinned { name: String },
    #[error("mod '{name}': hash mismatch: expected {expected}, got {got}")]
    HashMismatch {
        name: String,
        expected: String,
        got: String,
    },
    #[error("mod '{name}': archive missing from store: {path}")]
    StoreMiss { name: String, path: PathBuf },
    #[error("nexus api: {0}")]
    Nexus(String),
    #[error("no nexus api key — set NEXUS_API_KEY or ~/.config/fo4nix/nexus-api-key")]
    NoApiKey,
    #[error("http: {0}")]
    Http(Box<ureq::Error>),
    #[error("extraction failed for {archive}: {stderr}")]
    Extract { archive: PathBuf, stderr: String },
    #[error("instance path {0} refused: not a concierge-owned path")]
    UnsafeInstancePath(PathBuf),
    #[error("instance not materialized — run `concierge realize` first")]
    NoInstance,
    #[error("{0}")]
    Other(String),
}

impl From<ureq::Error> for Error {
    fn from(e: ureq::Error) -> Self {
        Self::Http(Box::new(e))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Convenience for attaching a path to an io error.
pub trait IoCtx<T> {
    fn ctx(self, path: &std::path::Path) -> Result<T>;
}

impl<T> IoCtx<T> for std::result::Result<T, std::io::Error> {
    fn ctx(self, path: &std::path::Path) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}
