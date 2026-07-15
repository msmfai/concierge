//! Realized state: exactly which files concierge owns (keyed by named
//! install root + relative path), and which pre-existing files it backed up
//! before overwriting.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{IoCtx, Result};
use crate::repo::Repo;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Realized {
    /// Hash of the Plan this state realizes (None after undeploy).
    #[serde(default)]
    pub plan_hash: Option<String>,
    /// Owned files keyed by "root:relpath".
    #[serde(default)]
    pub files: BTreeMap<String, OwnedFile>,
    /// Keys that had a pre-existing file backed up before overwrite.
    #[serde(default)]
    pub backups: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedFile {
    pub md5: String,
    pub mod_name: String,
}

pub fn key(root: &str, rel: &str) -> String {
    format!("{root}:{rel}")
}

pub fn parse_key(k: &str) -> Option<(&str, &str)> {
    k.split_once(':')
}

impl Realized {
    pub fn load(repo: &Repo) -> Result<Self> {
        let path = repo.state_file();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).ctx(&path)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, repo: &Repo) -> Result<()> {
        std::fs::create_dir_all(repo.state_dir()).ctx(&repo.state_dir())?;
        let path = repo.state_file();
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, text).ctx(&path)?;
        Ok(())
    }
}
