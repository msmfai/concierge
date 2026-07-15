//! Differential oracle for the config front-ends: the Nix-language tier and the
//! native TOML tier, given the *same* modpack, MUST evaluate to a byte-identical
//! plan. The TOML tier is the reference; Nix only borrows the language. Gated on
//! a Nix evaluator being present (the TOML tier needs nothing).
//!
//! The `nix=` tier is behind the `nix-source` feature, so this whole test only
//! compiles when built with `--features nix-source`.
#![cfg(feature = "nix-source")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::needless_raw_string_hashes
)]

use concierge::manifest::Manifest;
use concierge::plan::eval;

fn toml_src(pristine: &str, instance: &str) -> String {
    format!(
        r#"
[game]
kind = "custom"
pristine = "{pristine}"
instance = "{instance}"
version = "1.0"
[game.custom]
default_root = "data"
[[game.custom.root]]
name = "data"
dir = "Data"
[[mod]]
name = "bg3se"
version = "32"
install_root = "data"
subdir = "BG3Extender"
[[mod.pipeline]]
git = "https://github.com/Norbyte/bg3se@v32"
"#
    )
}

fn nix_src(lib: &str, pristine: &str, instance: &str) -> String {
    format!(
        r#"let lib = import {lib}; in
lib.mkModpack {{
  game = {{
    kind = "custom";
    pristine = "{pristine}";
    instance = "{instance}";
    version = "1.0";
    custom = {{ default_root = "data"; root = [ {{ name = "data"; dir = "Data"; }} ]; }};
  }};
  mods = [
    (lib.mkMod {{
      name = "bg3se"; version = "32"; install_root = "data"; subdir = "BG3Extender";
      source = lib.github "Norbyte/bg3se@v32";
    }})
  ];
}}
"#
    )
}

#[test]
fn nix_and_toml_front_ends_produce_identical_plan_hash() {
    if !concierge::nix::available() {
        eprintln!("nix evaluator not present; skipping differential (TOML tier still works)");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("cc-nixdiff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("pristine/Data")).unwrap();
    let pristine = tmp.join("pristine");
    let instance = tmp.join("instance");
    let (p, i) = (pristine.to_str().unwrap(), instance.to_str().unwrap());

    let lib = concat!(env!("CARGO_MANIFEST_DIR"), "/../../nix/lib.nix");
    let nixfile = tmp.join("modpack.nix");
    std::fs::write(&nixfile, nix_src(lib, p, i)).unwrap();

    let m_nix = concierge::nix::eval_manifest(&nixfile).expect("nix eval");
    let m_toml = Manifest::parse(&toml_src(p, i)).expect("toml parse");

    let h_nix = eval(&m_nix).unwrap().hash().unwrap();
    let h_toml = eval(&m_toml).unwrap().hash().unwrap();
    assert_eq!(
        h_nix, h_toml,
        "nix and toml front-ends must produce a byte-identical plan"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
