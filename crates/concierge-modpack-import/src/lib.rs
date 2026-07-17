//! Read-only importer for the `.wabbajack` modlist format — interop so a user
//! can turn a list they already have into a Concierge view (mods, sources,
//! hashes). The container is a ZIP whose `modlist` entry is the modlist JSON.
//!
//! Strictly user-local and read-only: it reads a file the user possesses and
//! reports what is in it; lists are never redistributed or hosted.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("not a .wabbajack: no `modlist`/`modlist.json` entry in the archive")]
    NoModlistEntry,
    #[error("zip: {0}")]
    Zip(String),
    #[error("json: {0}")]
    Json(String),
}

pub type Result<T> = std::result::Result<T, Error>;

// --- the raw schema we accept (a subset of Wabbajack.DTOs, by field name) ---

// Wabbajack serializes PascalCase; match those keys exactly.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawModList {
    #[serde(default)]
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    game_type: String,
    #[serde(default)]
    version: String,
    #[serde(default, rename = "IsNSFW")]
    is_nsfw: bool,
    #[serde(default)]
    website: String,
    #[serde(default)]
    archives: Vec<RawArchive>,
    #[serde(default)]
    directives: Vec<RawDirective>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawArchive {
    #[serde(default)]
    hash: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    state: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RawDirective {
    #[serde(default, rename = "$type")]
    kind: String,
}

// --- the Concierge-facing model ---

/// An imported modlist, mapped to Concierge's vocabulary.
#[derive(Debug, Clone)]
pub struct ModList {
    pub name: String,
    pub author: String,
    pub description: String,
    /// Wabbajack `GameType` (e.g. `Fallout4`); map to a Concierge game kind.
    pub game: String,
    pub version: String,
    pub is_nsfw: bool,
    pub website: String,
    pub archives: Vec<Archive>,
    /// Directive-kind histogram (we summarize install directives, not model all).
    pub directive_kinds: BTreeMap<String, usize>,
}

/// One source archive the list depends on.
#[derive(Debug, Clone)]
pub struct Archive {
    pub name: String,
    /// Wabbajack hash — base64 xxHash64 (NOT the md5 Concierge pins with).
    pub hash: String,
    pub size: u64,
    pub source: Source,
}

/// Where an archive is downloaded from (the `State.$type` discriminator).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Nexus {
        game: String,
        mod_id: u64,
        file_id: u64,
    },
    Http {
        url: String,
    },
    /// A source we recognize by name but don't model in detail
    /// (Mega/GoogleDrive/LoversLab/WabbajackCDN/GameFile/Manual/…).
    Other {
        kind: String,
    },
}

impl ModList {
    pub fn total_size(&self) -> u64 {
        self.archives.iter().map(|a| a.size).sum()
    }

    /// The archives that come from Nexus — these map directly onto Concierge
    /// `[[mod]]` entries (mod id + file id).
    pub fn nexus_mods(&self) -> Vec<&Archive> {
        self.archives
            .iter()
            .filter(|a| matches!(a.source, Source::Nexus { .. }))
            .collect()
    }

    /// Parse from a `.wabbajack` file (a ZIP with a `modlist` entry).
    pub fn from_modpack_archive(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(|source| Error::Io {
            path: path.display().to_string(),
            source,
        })?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Zip(e.to_string()))?;
        let entry_name = ["modlist", "modlist.json"]
            .into_iter()
            .find(|n| zip.by_name(n).is_ok())
            .ok_or(Error::NoModlistEntry)?;
        let mut entry = zip
            .by_name(entry_name)
            .map_err(|e| Error::Zip(e.to_string()))?;
        let mut json = Vec::new();
        entry.read_to_end(&mut json).map_err(|source| Error::Io {
            path: entry_name.to_owned(),
            source,
        })?;
        Self::from_modlist_json(&json)
    }

    /// Parse from the raw `modlist` JSON bytes (the entry inside the container).
    pub fn from_modlist_json(bytes: &[u8]) -> Result<Self> {
        let raw: RawModList =
            serde_json::from_slice(bytes).map_err(|e| Error::Json(e.to_string()))?;

        let archives = raw.archives.into_iter().map(map_archive).collect();
        let mut directive_kinds = BTreeMap::new();
        for d in raw.directives {
            let kind = d
                .kind
                .split(',')
                .next()
                .unwrap_or(&d.kind)
                .trim()
                .to_owned();
            *directive_kinds.entry(kind).or_insert(0) += 1;
        }
        Ok(Self {
            name: raw.name,
            author: raw.author,
            description: raw.description,
            game: raw.game_type,
            version: raw.version,
            is_nsfw: raw.is_nsfw,
            website: raw.website,
            archives,
            directive_kinds,
        })
    }
}

fn map_archive(raw: RawArchive) -> Archive {
    Archive {
        name: raw.name,
        hash: raw.hash,
        size: raw.size,
        source: map_source(&raw.state),
    }
}

/// Wabbajack game id -> Concierge adapter kind (best effort; unknown -> lower).
fn map_game(g: &str) -> String {
    match g {
        "Fallout4" => "fallout4".to_owned(),
        "SkyrimSpecialEdition" => "skyrimse".to_owned(),
        other => other.to_lowercase(),
    }
}

/// A TOML basic-string literal with `"` and `\` escaped.
fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl ModList {
    /// Render this modlist as a Concierge `manifest.toml` — a real, *evaluable*
    /// test case. Each archive becomes a `[[mod]]` carrying its **xxHash64** pin
    /// (`xxhash`): Nexus/Http keep their source; anything else becomes a manual
    /// (inbox) mod, hash-detected in `~/Downloads`. Paths are placeholders.
    ///
    /// NOTE: Wabbajack file-placement *directives* are not modelled, so this is
    /// a **structural** test case (eval / plan / sort / conflict-matrix over
    /// real sources + hashes), not a fully playable install.
    #[must_use]
    pub fn to_manifest_toml(&self) -> String {
        use std::fmt::Write as _;
        let kind = map_game(&self.game);
        let bethesda = matches!(kind.as_str(), "fallout4" | "skyrimse");
        let mut out = String::new();
        // writes to a String are infallible; discard the fmt::Result.
        let _ = writeln!(
            out,
            "# Imported from Wabbajack modlist: {} by {} (v{})",
            self.name, self.author, self.version
        );
        out.push_str(
            "# Structural test case (archives + sources + xxHash64 pins). Fill in paths.\n\n",
        );
        out.push_str("[game]\n");
        let _ = writeln!(out, "kind = {}", toml_str(&kind));
        out.push_str("pristine = \"/absolute/path/to/vanilla\"\n");
        out.push_str("instance = \"/absolute/path/to/instance\"\n");
        out.push_str("version = \"1.0\"\n");
        if bethesda {
            out.push_str("plugins_txt = \"./sandbox/AppData/plugins.txt\"\n");
            out.push_str("my_games = \"./sandbox/MyGames\"\n");
        }
        out.push('\n');
        for a in &self.archives {
            out.push_str("[[mod]]\n");
            let _ = writeln!(out, "name = {}", toml_str(&a.name));
            out.push_str("version = \"1\"\n");
            let _ = writeln!(out, "file = {}", toml_str(&a.name));
            let _ = writeln!(out, "xxhash = {}", toml_str(&a.hash));
            match &a.source {
                Source::Nexus {
                    mod_id, file_id, ..
                } => {
                    let _ = writeln!(out, "nexus_mod_id = {mod_id}");
                    let _ = writeln!(out, "nexus_file_id = {file_id}");
                }
                Source::Http { url } => {
                    let _ = writeln!(out, "url = {}", toml_str(url));
                }
                Source::Other { kind } => {
                    let _ = writeln!(
                        out,
                        "# manual source ({kind}): download to ~/Downloads; hash-detected via xxhash"
                    );
                }
            }
            out.push('\n');
        }
        out
    }
}

fn map_source(state: &serde_json::Value) -> Source {
    let type_tag = state
        .get("$type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let kind = type_tag.split(',').next().unwrap_or(type_tag).trim();
    let get_str = |k: &str| {
        state
            .get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let get_u64 = |k: &str| {
        state
            .get(k)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    if kind.contains("Nexus") {
        Source::Nexus {
            game: get_str("GameName"),
            mod_id: get_u64("ModID"),
            file_id: get_u64("FileID"),
        }
    } else if kind.contains("Http") {
        Source::Http {
            url: get_str("Url"),
        }
    } else {
        Source::Other {
            kind: kind.to_owned(),
        }
    }
}
