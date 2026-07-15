//! Native parser for the CC0 LOOT masterlist (the metadata LOOT itself
//! consumes). We read only what a load-order sort and dirty/tag surfacing
//! need: per-plugin `after`/`req`/`group`/`tag`/`dirty`, and the group graph.
//! Anchors resolve within the cached single-document masterlist; the `<<`
//! merge key appears as an ignored mapping key.

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Masterlist {
    #[serde(default)]
    pub groups: Vec<GroupDef>,
    #[serde(default)]
    pub plugins: Vec<PluginMeta>,
}

#[derive(Debug, Deserialize)]
pub struct GroupDef {
    pub name: String,
    #[serde(default)]
    pub after: Vec<StrOr>,
}

#[derive(Debug, Deserialize)]
pub struct PluginMeta {
    pub name: String,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub after: Vec<StrOr>,
    #[serde(default)]
    pub req: Vec<StrOr>,
    #[serde(default)]
    pub tag: Vec<StrOr>,
    #[serde(default)]
    pub dirty: Vec<DirtyInfo>,
}

#[derive(Debug, Deserialize)]
pub struct DirtyInfo {
    /// CRC-32 of the plugin before cleaning (YAML `0x…` int).
    #[serde(default)]
    pub crc: Option<u64>,
    #[serde(default)]
    pub itm: u64,
    #[serde(default)]
    pub udr: u64,
    #[serde(default)]
    pub nav: u64,
}

/// A masterlist reference is either a bare string or a `{name, …}` map.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum StrOr {
    Str(String),
    Map {
        name: String,
        #[serde(default)]
        #[allow(dead_code)]
        display: Option<String>,
    },
}

impl StrOr {
    pub fn name(&self) -> &str {
        match self {
            Self::Str(s) | Self::Map { name: s, .. } => s,
        }
    }
}

impl Masterlist {
    pub fn parse(yaml: &str) -> Result<Self, String> {
        serde_yaml::from_str(yaml).map_err(|e| e.to_string())
    }

    /// Metadata for a plugin, matching literal names case-insensitively and
    /// regex `name` entries against the filename.
    pub fn for_plugin(&self, filename: &str) -> Vec<&PluginMeta> {
        self.plugins
            .iter()
            .filter(|p| name_matches(&p.name, filename))
            .collect()
    }
}

/// A masterlist `name` is a regex when it contains regex metacharacters;
/// otherwise a case-insensitive literal (LOOT's rule). Regex names are
/// anchored `^…$`, case-insensitive (the `regex` crate — MIT/Apache).
fn name_matches(pattern: &str, filename: &str) -> bool {
    let is_regex = pattern.bytes().any(|b| {
        matches!(
            b,
            b'[' | b']'
                | b'.'
                | b'*'
                | b'+'
                | b'\\'
                | b'?'
                | b'('
                | b')'
                | b'|'
                | b'^'
                | b'$'
                | b'{'
                | b'}'
        )
    });
    if !is_regex {
        return pattern.eq_ignore_ascii_case(filename);
    }
    regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
        .case_insensitive(true)
        .build()
        .is_ok_and(|re| re.is_match(filename))
}
