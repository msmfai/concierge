//! FOMOD installer — the model Vortex, Mod Organizer 2, and Wabbajack use.
//!
//! A FOMOD archive carries `fomod/ModuleConfig.xml` describing an installer:
//! files that are always installed, plus ordered steps of option groups whose
//! selected options contribute their own files and set condition flags, plus
//! flag-conditional installs. The manager records the user's picks and copies
//! ONLY the selected `source → destination` files. This crate is the pure
//! parse + resolve half: [`parse`] reads the XML into [`FomodConfig`], and
//! [`FomodConfig::resolve`] turns a set of picked option names into the exact
//! ordered [`InstallItem`]s to deploy. No bulk-deploy, no folder guessing.

use std::collections::{HashMap, HashSet};

/// A single install directive: copy `source` (archive-relative) to
/// `destination` (Data-relative). `is_folder` means recurse. Paths use forward
/// slashes; the archive lookup is case-insensitive (FOMOD convention).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallItem {
    pub source: String,
    pub destination: String,
    pub is_folder: bool,
    pub priority: i32,
}

/// The parsed installer.
#[derive(Debug, Clone, Default)]
pub struct FomodConfig {
    pub module_name: String,
    /// Always installed, before any option.
    pub required: Vec<InstallItem>,
    pub steps: Vec<InstallStep>,
    /// Flag-conditioned installs applied after all option files.
    pub conditional: Vec<ConditionalInstall>,
}

#[derive(Debug, Clone)]
pub struct InstallStep {
    pub name: String,
    /// Step is only presented when this condition holds (None = always).
    pub visible: Option<Condition>,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub name: String,
    pub kind: GroupKind,
    pub options: Vec<Opt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    /// radio, exactly one
    ExactlyOne,
    /// radio, zero or one
    AtMostOne,
    /// checkbox, one or more
    AtLeastOne,
    /// checkbox, zero or more
    Any,
    /// all forced on
    All,
}

/// One selectable option (`<plugin>` in the schema).
#[derive(Debug, Clone)]
pub struct Opt {
    pub name: String,
    pub description: String,
    pub files: Vec<InstallItem>,
    /// Flags this option sets when selected (`conditionFlags`).
    pub flags: Vec<(String, String)>,
    pub kind: OptType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptType {
    Required,
    Recommended,
    Optional,
    NotUsable,
    CouldBeUsable,
}

#[derive(Debug, Clone)]
pub struct ConditionalInstall {
    pub condition: Condition,
    pub files: Vec<InstallItem>,
}

/// A flag condition (`dependencies` with `flagDependency` children). File
/// dependencies are treated as satisfied — we can't know the user's other mods
/// here, and the manager records the resulting selection anyway.
#[derive(Debug, Clone)]
pub struct Condition {
    pub operator: Operator,
    pub flags: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    And,
    Or,
}

impl Condition {
    #[must_use]
    pub fn eval(&self, flags: &HashMap<String, String>) -> bool {
        if self.flags.is_empty() {
            return true;
        }
        let hit = |(k, v): &(String, String)| flags.get(k).is_some_and(|cur| cur == v);
        match self.operator {
            Operator::And => self.flags.iter().all(hit),
            Operator::Or => self.flags.iter().any(hit),
        }
    }
}

/// Decode a `ModuleConfig.xml` byte blob — FOMODs are commonly UTF-16 (with
/// BOM), sometimes UTF-8 — into a string, then [`parse_str`].
///
/// # Errors
/// Returns the roxmltree parse error message if the XML is malformed.
pub fn parse(bytes: &[u8]) -> Result<FomodConfig, String> {
    parse_str(&decode(bytes))
}

/// Decode bytes to a String, honoring a UTF-16 LE/BE or UTF-8 BOM, else UTF-8.
#[must_use]
pub fn decode(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => decode_utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => decode_utf16(rest, true),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn decode_utf16(bytes: &[u8], big_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .filter_map(|c| match *c {
            [a, b] if big_endian => Some(u16::from_be_bytes([a, b])),
            [a, b] => Some(u16::from_le_bytes([a, b])),
            _ => None,
        })
        .collect();
    String::from_utf16_lossy(&units)
}

fn norm(path: &str) -> String {
    path.trim().replace('\\', "/").trim_matches('/').to_owned()
}

/// Parse an already-decoded ModuleConfig.xml string.
///
/// # Errors
/// Returns the parse error message if the XML is malformed.
pub fn parse_str(xml: &str) -> Result<FomodConfig, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let mut cfg = FomodConfig::default();

    for child in root.children().filter(roxmltree::Node::is_element) {
        match child.tag_name().name() {
            "moduleName" => {
                child
                    .text()
                    .unwrap_or_default()
                    .trim()
                    .clone_into(&mut cfg.module_name);
            }
            "requiredInstallFiles" => cfg.required = parse_files(child),
            "installSteps" => {
                for step in child.children().filter(|n| n.has_tag_name("installStep")) {
                    cfg.steps.push(parse_step(step));
                }
            }
            "conditionalFileInstalls" => {
                if let Some(patterns) = child.children().find(|n| n.has_tag_name("patterns")) {
                    for pat in patterns.children().filter(|n| n.has_tag_name("pattern")) {
                        let condition = pat
                            .children()
                            .find(|n| n.has_tag_name("dependencies"))
                            .map_or_else(
                                || Condition {
                                    operator: Operator::And,
                                    flags: Vec::new(),
                                },
                                parse_condition,
                            );
                        let files = pat
                            .children()
                            .find(|n| n.has_tag_name("files"))
                            .map(parse_files)
                            .unwrap_or_default();
                        cfg.conditional
                            .push(ConditionalInstall { condition, files });
                    }
                }
            }
            _ => {}
        }
    }
    Ok(cfg)
}

/// Parse a `<files>` / `<requiredInstallFiles>` container of `<file>`/`<folder>`.
fn parse_files(node: roxmltree::Node) -> Vec<InstallItem> {
    node.children()
        .filter(roxmltree::Node::is_element)
        .filter_map(|n| {
            let tag = n.tag_name().name();
            let is_folder = match tag {
                "folder" => true,
                "file" => false,
                _ => return None,
            };
            let source = norm(n.attribute("source")?);
            // FOMOD destination semantics: ABSENT means "install in place" (same
            // as source); PRESENT-but-empty ("") means the game data ROOT. These
            // differ — a folder `source="00 - Core Files" destination=""` installs
            // its CONTENTS at Data/, not Data/00 - Core Files/.
            let destination = match n.attribute("destination") {
                None => source.clone(),
                Some(d) => norm(d),
            };
            let priority = n
                .attribute("priority")
                .and_then(|p| p.trim().parse().ok())
                .unwrap_or(0);
            Some(InstallItem {
                source,
                destination,
                is_folder,
                priority,
            })
        })
        .collect()
}

fn parse_step(node: roxmltree::Node) -> InstallStep {
    let visible = node
        .children()
        .find(|n| n.has_tag_name("visible"))
        .and_then(|v| v.children().find(|n| n.has_tag_name("dependencies")))
        .map(parse_condition);
    let mut groups = Vec::new();
    if let Some(ofg) = node
        .children()
        .find(|n| n.has_tag_name("optionalFileGroups"))
    {
        for g in ofg.children().filter(|n| n.has_tag_name("group")) {
            groups.push(parse_group(g));
        }
    }
    InstallStep {
        name: node.attribute("name").unwrap_or_default().to_owned(),
        visible,
        groups,
    }
}

fn parse_group(node: roxmltree::Node) -> Group {
    let kind = match node.attribute("type").unwrap_or("SelectAny") {
        "SelectExactlyOne" => GroupKind::ExactlyOne,
        "SelectAtMostOne" => GroupKind::AtMostOne,
        "SelectAtLeastOne" => GroupKind::AtLeastOne,
        "SelectAll" => GroupKind::All,
        _ => GroupKind::Any,
    };
    let mut options = Vec::new();
    if let Some(plugins) = node.children().find(|n| n.has_tag_name("plugins")) {
        for p in plugins.children().filter(|n| n.has_tag_name("plugin")) {
            options.push(parse_option(p));
        }
    }
    Group {
        name: node.attribute("name").unwrap_or_default().to_owned(),
        kind,
        options,
    }
}

fn parse_option(node: roxmltree::Node) -> Opt {
    let files = node
        .children()
        .find(|n| n.has_tag_name("files"))
        .map(parse_files)
        .unwrap_or_default();
    let flags = node
        .children()
        .find(|n| n.has_tag_name("conditionFlags"))
        .map(|cf| {
            cf.children()
                .filter(|n| n.has_tag_name("flag"))
                .filter_map(|f| {
                    Some((
                        f.attribute("name")?.to_owned(),
                        f.text().unwrap_or("").trim().to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    // typeDescriptor: a plain <type name="..."/> or a <dependencyType> whose
    // <defaultType> we take (pattern-conditioned types resolve to the default
    // for our non-interactive defaulting).
    let kind = node
        .children()
        .find(|n| n.has_tag_name("typeDescriptor"))
        .and_then(|td| {
            td.children()
                .find(|n| n.has_tag_name("type"))
                .or_else(|| {
                    td.children()
                        .find(|n| n.has_tag_name("dependencyType"))
                        .and_then(|dt| dt.children().find(|n| n.has_tag_name("defaultType")))
                })
                .and_then(|t| t.attribute("name"))
        })
        .map_or(OptType::Optional, |name| match name {
            "Required" => OptType::Required,
            "Recommended" => OptType::Recommended,
            "NotUsable" => OptType::NotUsable,
            "CouldBeUsable" => OptType::CouldBeUsable,
            _ => OptType::Optional,
        });
    Opt {
        name: node.attribute("name").unwrap_or_default().trim().to_owned(),
        description: node
            .children()
            .find(|n| n.has_tag_name("description"))
            .and_then(|d| d.text())
            .unwrap_or_default()
            .trim()
            .to_owned(),
        files,
        flags,
        kind,
    }
}

fn parse_condition(node: roxmltree::Node) -> Condition {
    let operator = match node.attribute("operator") {
        Some("Or") => Operator::Or,
        _ => Operator::And,
    };
    let flags = node
        .children()
        .filter(|n| n.has_tag_name("flagDependency"))
        .filter_map(|f| {
            Some((
                f.attribute("flag")?.to_owned(),
                f.attribute("value")?.to_owned(),
            ))
        })
        .collect();
    Condition { operator, flags }
}

impl FomodConfig {
    /// Every option name across all groups, in presentation order — the domain
    /// of a selection.
    #[must_use]
    pub fn option_names(&self) -> Vec<String> {
        self.steps
            .iter()
            .flat_map(|s| s.groups.iter())
            .flat_map(|g| g.options.iter())
            .map(|o| o.name.clone())
            .collect()
    }

    /// Resolve recorded selections into the ordered install items. An option is
    /// installed if it is selected by name, is `Required`, or lives in a
    /// `SelectAll` group. Selected options set their flags (in step order), and
    /// flag-conditional installs are appended once all flags are known. Steps
    /// hidden by an unmet `visible` condition are skipped.
    #[must_use]
    pub fn resolve(&self, selected: &HashSet<String>) -> Vec<InstallItem> {
        let mut flags: HashMap<String, String> = HashMap::new();
        let mut items = self.required.clone();
        for step in &self.steps {
            if step.visible.as_ref().is_some_and(|c| !c.eval(&flags)) {
                continue;
            }
            for group in &step.groups {
                for opt in &group.options {
                    let on = opt.kind == OptType::Required
                        || group.kind == GroupKind::All
                        || selected.contains(&opt.name);
                    if on && opt.kind != OptType::NotUsable {
                        items.extend(opt.files.iter().cloned());
                        for (k, v) in &opt.flags {
                            flags.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
        }
        for c in &self.conditional {
            if c.condition.eval(&flags) {
                items.extend(c.files.iter().cloned());
            }
        }
        items.sort_by_key(|i| i.priority);
        items
    }

    /// The effective selection given the user's explicit picks, merged OVER the
    /// installer defaults PER GROUP — exactly how a real installer behaves: each
    /// group starts on its recommended default, and an explicit pick only
    /// overrides the group it belongs to (replacing that group's default),
    /// leaving every other group on its default. So `select = ["VIS sorting"]`
    /// on a big installer changes just the sorting group and keeps sensible
    /// defaults everywhere else. Names matching no option are ignored here (a
    /// separate lint flags them).
    #[must_use]
    pub fn selection_merged(&self, explicit: &HashSet<String>) -> HashSet<String> {
        let defaults = self.default_selection();
        let mut sel = HashSet::new();
        for step in &self.steps {
            for group in &step.groups {
                let picked_here: Vec<&Opt> = group
                    .options
                    .iter()
                    .filter(|o| explicit.contains(&o.name))
                    .collect();
                if picked_here.is_empty() {
                    // no override for this group — keep its defaults
                    for o in &group.options {
                        if defaults.contains(&o.name) {
                            sel.insert(o.name.clone());
                        }
                    }
                } else {
                    for o in picked_here {
                        sel.insert(o.name.clone());
                    }
                }
            }
        }
        sel
    }

    /// A sane non-interactive default selection — the equivalent of clicking
    /// through the installer taking the author's recommendations: every
    /// `Required`/`Recommended` option, plus the first usable option of a
    /// `SelectExactlyOne`/`SelectAtLeastOne` group that has no recommendation.
    ///
    /// Also honors visibility gating: an installer that hides its main step
    /// behind an "I agree / I understand" acknowledgement (Functional Displays)
    /// won't install anything unless that box is checked. Such an option
    /// installs no files and only sets a flag that unlocks a later step, so we
    /// select it by default — but NOT a no-file option whose flag merely drives
    /// content installs (a "DLC: Far Harbor" toggle), which would add content
    /// the user may not want.
    #[must_use]
    pub fn default_selection(&self) -> HashSet<String> {
        // Flag=value pairs that gate a step's visibility — the acknowledgement
        // gates worth auto-satisfying (distinct from conditionalFileInstalls).
        let gate_flags: std::collections::HashSet<(String, String)> = self
            .steps
            .iter()
            .filter_map(|s| s.visible.as_ref())
            .flat_map(|c| c.flags.iter().cloned())
            .collect();

        let mut sel = HashSet::new();
        for step in &self.steps {
            for group in &step.groups {
                let usable = |o: &&Opt| o.kind != OptType::NotUsable;
                let recommended: Vec<&Opt> = group
                    .options
                    .iter()
                    .filter(|o| matches!(o.kind, OptType::Required | OptType::Recommended))
                    .collect();
                let take: Vec<&Opt> = if recommended.is_empty() {
                    match group.kind {
                        GroupKind::ExactlyOne | GroupKind::AtLeastOne => {
                            group.options.iter().find(usable).into_iter().collect()
                        }
                        GroupKind::All => group.options.iter().collect(),
                        GroupKind::Any | GroupKind::AtMostOne => Vec::new(),
                    }
                } else {
                    recommended
                };
                for o in take {
                    sel.insert(o.name.clone());
                }
                // Acknowledgement gates: a no-file option whose flag unlocks a
                // later step. Safe to enable (installs nothing) and required to
                // reach the gated content.
                for o in &group.options {
                    if o.kind != OptType::NotUsable
                        && o.files.is_empty()
                        && o.flags.iter().any(|f| gate_flags.contains(f))
                    {
                        sel.insert(o.name.clone());
                    }
                }
            }
        }
        sel
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sel(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    const SURVIVAL_OPTIONS: &str = r#"<config>
      <moduleName>Survival Options</moduleName>
      <requiredInstallFiles>
        <file source="SurvivalOptions - Main.ba2" destination="SurvivalOptions - Main.ba2" />
        <folder source="MCM" destination="MCM" />
      </requiredInstallFiles>
      <installSteps order="Explicit">
        <installStep name="Step1">
          <optionalFileGroups order="Explicit">
            <group name="Main" type="SelectExactlyOne">
              <plugins order="Explicit">
                <plugin name="Everything">
                  <description>all on</description>
                  <files><file source="SurvivalOptions_Everything\SurvivalOptions.esp" destination="SurvivalOptions.esp" /></files>
                  <typeDescriptor><type name="Optional"/></typeDescriptor>
                </plugin>
                <plugin name="None">
                  <description>none</description>
                  <files><file source="SurvivalOptions_None\SurvivalOptions.esp" destination="SurvivalOptions.esp" /></files>
                  <typeDescriptor><type name="Recommended"/></typeDescriptor>
                </plugin>
              </plugins>
            </group>
          </optionalFileGroups>
        </installStep>
      </installSteps>
    </config>"#;

    #[test]
    fn parses_and_resolves_select_exactly_one() {
        let cfg = parse_str(SURVIVAL_OPTIONS).unwrap();
        assert_eq!(cfg.module_name, "Survival Options");
        assert_eq!(cfg.required.len(), 2);
        assert!(cfg
            .required
            .iter()
            .any(|i| i.is_folder && i.destination == "MCM"));

        let items = cfg.resolve(&sel(&["Everything"]));
        // required (2) + the one picked esp
        assert_eq!(items.len(), 3);
        let esp = items
            .iter()
            .find(|i| i.destination == "SurvivalOptions.esp")
            .unwrap();
        assert_eq!(esp.source, "SurvivalOptions_Everything/SurvivalOptions.esp");
        // the OTHER option's file must NOT be present
        assert!(!items.iter().any(|i| i.source.contains("_None")));
    }

    #[test]
    fn default_selection_takes_the_recommendation() {
        let cfg = parse_str(SURVIVAL_OPTIONS).unwrap();
        let d = cfg.default_selection();
        assert!(d.contains("None"), "Recommended option defaulted on");
        let items = cfg.resolve(&d);
        assert!(items
            .iter()
            .any(|i| i.source.contains("_None/SurvivalOptions.esp")));
    }

    #[test]
    fn flags_drive_conditional_installs() {
        let xml = r#"<config><moduleName>M</moduleName>
          <installSteps><installStep name="s">
            <optionalFileGroups>
              <group name="g" type="SelectAny">
                <plugins>
                  <plugin name="DLC Patch">
                    <files><file source="patch.esp" destination="patch.esp"/></files>
                    <conditionFlags><flag name="wantPatch">On</flag></conditionFlags>
                    <typeDescriptor><type name="Optional"/></typeDescriptor>
                  </plugin>
                </plugins>
              </group>
            </optionalFileGroups>
          </installStep></installSteps>
          <conditionalFileInstalls><patterns>
            <pattern>
              <dependencies operator="And"><flagDependency flag="wantPatch" value="On"/></dependencies>
              <files><file source="extra.esp" destination="extra.esp"/></files>
            </pattern>
          </patterns></conditionalFileInstalls>
        </config>"#;
        let cfg = parse_str(xml).unwrap();
        // not selected -> flag unset -> no conditional file
        assert!(cfg.resolve(&sel(&[])).is_empty());
        // selected -> flag set -> both the option file and the conditional file
        let items = cfg.resolve(&sel(&["DLC Patch"]));
        assert!(items.iter().any(|i| i.destination == "patch.esp"));
        assert!(
            items.iter().any(|i| i.destination == "extra.esp"),
            "conditional install fired"
        );
    }

    #[test]
    fn empty_destination_means_data_root_not_source() {
        // `destination=""` (present, empty) installs contents at the Data ROOT;
        // an ABSENT destination installs in place. True Storms' core folder
        // relies on the former.
        let xml = r#"<config><moduleName>M</moduleName>
          <requiredInstallFiles>
            <folder source="00 - Core Files" destination="" />
            <file source="loose/x.esp" />
          </requiredInstallFiles>
        </config>"#;
        let cfg = parse_str(xml).unwrap();
        let core = cfg
            .required
            .iter()
            .find(|i| i.source == "00 - Core Files")
            .unwrap();
        assert_eq!(core.destination, "", "empty destination -> Data root");
        let loose = cfg
            .required
            .iter()
            .find(|i| i.source == "loose/x.esp")
            .unwrap();
        assert_eq!(
            loose.destination, "loose/x.esp",
            "absent destination -> in place"
        );
    }

    #[test]
    fn default_selection_satisfies_acknowledgement_gates() {
        // Functional Displays shape: a required main step hidden until an
        // "I agree" option (no files, sets a flag) is checked.
        let xml = r#"<config><moduleName>M</moduleName>
          <installSteps>
            <installStep name="Welcome">
              <optionalFileGroups><group name="Terms" type="SelectAtMostOne"><plugins>
                <plugin name="I agree">
                  <conditionFlags><flag name="agreed">yes</flag></conditionFlags>
                  <typeDescriptor><type name="Optional"/></typeDescriptor>
                </plugin>
              </plugins></group></optionalFileGroups>
            </installStep>
            <installStep name="Main">
              <visible><dependencies operator="And"><flagDependency flag="agreed" value="yes"/></dependencies></visible>
              <optionalFileGroups><group name="Core" type="SelectExactlyOne"><plugins>
                <plugin name="Main File">
                  <files><file source="Main.esp" destination="Main.esp"/></files>
                  <typeDescriptor><type name="Required"/></typeDescriptor>
                </plugin>
              </plugins></group></optionalFileGroups>
            </installStep>
          </installSteps>
        </config>"#;
        let cfg = parse_str(xml).unwrap();
        let d = cfg.default_selection();
        assert!(d.contains("I agree"), "acknowledgement gate auto-selected");
        // ...so the gated required content actually installs.
        let items = cfg.resolve(&d);
        assert!(
            items.iter().any(|i| i.destination == "Main.esp"),
            "gated required file installs"
        );
    }

    #[test]
    fn explicit_pick_overrides_only_its_group() {
        // Two groups; explicitly pick the non-default sort. The other group must
        // keep its default (recommended) pick — not be dropped.
        let xml = r#"<config><moduleName>M</moduleName>
          <installSteps><installStep name="s"><optionalFileGroups>
            <group name="Ratio" type="SelectExactlyOne"><plugins>
              <plugin name="16:9"><files><file source="a" destination="a"/></files>
                <typeDescriptor><type name="Recommended"/></typeDescriptor></plugin>
              <plugin name="4:3"><files><file source="b" destination="b"/></files>
                <typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
            </plugins></group>
            <group name="Sort" type="SelectExactlyOne"><plugins>
              <plugin name="Vanilla"><files><file source="c" destination="c"/></files>
                <typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
              <plugin name="VIS"><files><file source="d" destination="d"/></files>
                <typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
            </plugins></group>
          </optionalFileGroups></installStep></installSteps>
        </config>"#;
        let cfg = parse_str(xml).unwrap();
        let sel = cfg.selection_merged(&sel(&["VIS"]));
        assert!(sel.contains("VIS"), "explicit sort pick honored");
        assert!(
            sel.contains("16:9"),
            "the OTHER group keeps its recommended default"
        );
        assert!(
            !sel.contains("Vanilla"),
            "explicit pick replaced the default in its own group"
        );
        assert!(!sel.contains("4:3"));
    }

    #[test]
    fn default_selection_ignores_content_toggles() {
        // A no-file option whose flag drives a conditional CONTENT install (not
        // a visibility gate) must NOT be auto-selected — that would add DLC the
        // user may not have.
        let xml = r#"<config><moduleName>M</moduleName>
          <installSteps><installStep name="s">
            <optionalFileGroups><group name="DLC" type="SelectAny"><plugins>
              <plugin name="Far Harbor">
                <conditionFlags><flag name="fh">on</flag></conditionFlags>
                <typeDescriptor><type name="Optional"/></typeDescriptor>
              </plugin>
            </plugins></group></optionalFileGroups>
          </installStep></installSteps>
          <conditionalFileInstalls><patterns><pattern>
            <dependencies><flagDependency flag="fh" value="on"/></dependencies>
            <files><file source="fh.esp" destination="fh.esp"/></files>
          </pattern></patterns></conditionalFileInstalls>
        </config>"#;
        let cfg = parse_str(xml).unwrap();
        assert!(
            !cfg.default_selection().contains("Far Harbor"),
            "content toggle not auto-selected"
        );
    }

    #[test]
    fn utf16_bom_decodes() {
        let mut bytes = vec![0xFF, 0xFE];
        for u in "<config><moduleName>Z</moduleName></config>".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(parse(&bytes).unwrap().module_name, "Z");
    }
}
