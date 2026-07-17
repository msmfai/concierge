//! Hermetic tests for the native sort-rule parser + name matcher.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use concierge_pluginorder::sortrules::SortRules;

const YAML: &str = r"
common:
  - &qc
    util: 'FO4Edit'
groups:
  - name: 'default'
  - name: 'CC'
    after: [ 'default' ]
plugins:
  - name: 'DLCCoast.esm'
    after: [ 'DLCworkshop01.esm' ]
    dirty:
      - <<: *qc
        crc: 0xF1F28026
        itm: 83
        udr: 86
  - name: 'DLCNukaWorld.esm'
    tag: [ Relev ]
  - name: 'cc[A-Z]{3}FO4[0-9]{3}.*\.es(l|m)'
    group: 'CC'
";

#[test]
fn parses_plugins_groups_dirty_tags() {
    let ml = SortRules::parse(YAML).unwrap();
    assert_eq!(ml.groups.len(), 2);
    assert_eq!(ml.plugins.len(), 3);
    let coast = ml.for_plugin("DLCCoast.esm");
    assert_eq!(coast.len(), 1);
    assert_eq!(coast[0].after[0].name(), "DLCworkshop01.esm");
    assert_eq!(coast[0].dirty[0].itm, 83);
    assert_eq!(coast[0].dirty[0].udr, 86);
    assert_eq!(coast[0].dirty[0].crc, Some(0xF1F2_8026));
    let nuka = ml.for_plugin("DLCNukaWorld.esm");
    assert_eq!(nuka[0].tag[0].name(), "Relev");
}

#[test]
fn regex_names_match_creation_club() {
    let ml = SortRules::parse(YAML).unwrap();
    let cc = ml.for_plugin("ccBGSFO4115-X02.esl");
    assert_eq!(cc.len(), 1);
    assert_eq!(cc[0].group.as_deref(), Some("CC"));
    assert!(ml
        .for_plugin("DLCRobot.esm")
        .iter()
        .all(|p| p.group.as_deref() != Some("CC")));
    assert_eq!(ml.for_plugin("dlccoast.esm").len(), 1);
}
