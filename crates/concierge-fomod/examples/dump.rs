//! Spot-check the parser against a real ModuleConfig.xml:
//! `cargo run -p concierge-fomod --example dump -- <path>`
#![allow(
    clippy::print_stdout,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
fn main() {
    let path = std::env::args().nth(1).expect("path arg");
    let bytes = std::fs::read(&path).unwrap();
    let cfg = concierge_fomod::parse(&bytes).unwrap_or_else(|e| panic!("parse {path}: {e}"));
    println!("module: {}", cfg.module_name);
    println!("required: {}", cfg.required.len());
    println!(
        "steps: {}  conditional: {}",
        cfg.steps.len(),
        cfg.conditional.len()
    );
    let opts = cfg.option_names();
    println!("options: {}", opts.len());
    // --select "A" "B" ... overrides the default; otherwise use defaults.
    let mut args = std::env::args().skip_while(|a| a != "--select");
    let picks: std::collections::HashSet<String> = args
        .by_ref()
        .skip(1)
        .take_while(|a| !a.starts_with("--"))
        .collect();
    let d = cfg.selection_merged(&picks);
    let items = cfg.resolve(&d);
    println!(
        "selection: {} opts -> {} install items",
        d.len(),
        items.len()
    );
    for i in &items {
        println!(
            "   {} {} -> {}",
            if i.is_folder { "DIR " } else { "file" },
            i.source,
            i.destination
        );
    }
    if std::env::args().any(|a| a == "--tree") {
        for step in &cfg.steps {
            println!("STEP {}", step.name);
            for g in &step.groups {
                println!("  GROUP [{:?}] {}", g.kind, g.name);
                for o in &g.options {
                    let files: Vec<&str> = o.files.iter().map(|f| f.destination.as_str()).collect();
                    println!("    - {:?}  \"{}\"  -> {:?}", o.kind, o.name, files);
                }
            }
        }
    }
}
