use clap::{Parser, Subcommand};

mod headless;
mod init;

use concierge::build::{build_all, BuildOutcome};
use concierge::check::{check, write_vanilla_inventory, Drift};
use concierge::error::{IoCtx as _, Result};
use concierge::launch::{deactivate, launch};
use concierge::manifest::Manifest;
use concierge::plan::eval;
use concierge::realize::{realize, undeploy};
use concierge::repo::Repo;
use concierge::state::Realized;
use concierge::store::{fetch_all, FetchOutcome};
use concierge::{nexus, Error};

#[derive(Parser)]
#[command(
    name = "concierge-cli",
    version,
    about = "Declarative mod management — eval a pure plan, realize it into the game instance"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold an agent-ready profile folder (config + sandbox + CLAUDE.md + skills)
    Init {
        /// Game kind (fallout4, bg3, kotor2, skyrimse, custom, …)
        #[arg(long, default_value = "fallout4")]
        game: String,
        /// Use the Nix config tier (modpack.nix) instead of manifest.toml
        #[arg(long)]
        nix: bool,
        /// Target directory (default: current directory)
        dir: Option<String>,
    },
    /// Adopt a game concierge doesn't know: scaffold games/<NAME>/profiles/default
    /// from where the game is and (optionally) how modding works. No modding
    /// flags = the generic case (mods just add/replace files, no assumptions).
    /// The automation surface behind the GUI's add-game wizard — agents included.
    Adopt {
        /// Directory name for the game (becomes games/<NAME>/)
        name: String,
        /// Absolute path to the game install
        #[arg(long)]
        pristine: String,
        /// Game version string for the manifest
        #[arg(long, default_value = "unknown")]
        game_version: String,
        /// Mods live in this dir INSIDE the game tree (relative, e.g. "Mods")
        #[arg(long)]
        mods_dir: Option<String>,
        /// Mods live at this EXTERNAL absolute path (deployed as a mount)
        #[arg(long)]
        mods_path: Option<String>,
        /// Launch candidate (repeatable; app bundle or exe name)
        #[arg(long)]
        launch: Vec<String>,
        /// Nexus domain, when the game has a Nexus community
        #[arg(long)]
        nexus_domain: Option<String>,
        #[arg(long)]
        steam_app_id: Option<u32>,
    },
    /// Evaluate the manifest into a pure, hashed Plan (writes state/plan.json)
    Eval,
    /// Headless agent view: print the text/state-automaton rendering of the GUI
    /// (no window), or run a step script (file path, or `-` for stdin).
    Tui {
        /// A step script to run: snapshot / click <id> / tab <name> / type
        /// <field> <val> / assert state=<X> / assert intent <id> <cond>.
        #[arg(long)]
        script: Option<String>,
        /// Run read-only heavy checks (eval + health GO/NO-GO) against the
        /// profile. Still no deploy / network — the `health` step + `assert
        /// health go|no-go` become available.
        #[arg(long)]
        live: bool,
    },
    /// Fetch archives into the content-addressed store (TOFU-pins print here)
    Fetch,
    /// Extract fetched archives into immutable build trees
    Build,
    /// Apply your setup to the game (fetch + build + deploy). Alias: `apply`.
    #[command(visible_alias = "apply")]
    Realize {
        /// Delete and re-clone the instance from pristine first
        #[arg(long)]
        fresh: bool,
        /// After deploying, sort the load order (LOOT) — the correct final step,
        /// so "sort must be last" isn't a footgun. Full converge in one command.
        #[arg(long)]
        sort: bool,
    },
    /// Drift detection against realized state
    Check {
        /// Also verify the pristine install against the vanilla inventory
        #[arg(long)]
        vanilla: bool,
    },
    /// Dry-run: what each mod WOULD deploy + activate, without deploying
    Preview {
        /// List every destination file (default: per-mod counts + activations)
        #[arg(long)]
        files: bool,
    },
    /// Activation truth: active entries (in order), deployed-but-inactive
    /// (inert), and unresolved dependencies — driven by the plan, not grep
    Plugins,
    /// One health report: pins, invariant lints, inert plugins, empty dirs,
    /// drift, and pristine-safety — pass/fail per check (nonzero exit on fail)
    Doctor,
    /// Make profile folders agent-ready (CLAUDE.md guide + slash-commands +
    /// permissions allowlist), only writing what's absent. With --all,
    /// provisions every games/*/profiles/* under the workspace.
    Provision {
        /// Provision every profile in the workspace, not just the active one
        #[arg(long)]
        all: bool,
    },
    /// Verify every declared `nexus_mod_id` against the synced catalog — OK /
    /// NAME MISMATCH / UNKNOWN ID — so an invented or mistyped id is loud, not
    /// silently trusted. Records results in state/audit.json; `eval` and
    /// `realize` warn about unaudited Nexus entries.
    Audit,
    /// Lock the profile: the declaration (manifest + lockfile) becomes
    /// read-only ON DISK — the GUI, agents, editors, and the sandbox all
    /// honor the same fact. Undo with `unlock`.
    Lock,
    /// Unlock the profile: the declaration becomes editable again.
    Unlock,
    /// Sandboxed shell/agent for this profile: writes are OS-confined to the
    /// plan's write-set (store/builds/state, profile, instance, mounts,
    /// configs); the pristine install is read-only even here. Run your agent
    /// inside it (`--agent claude`) — the blast radius is Concierge, not the
    /// machine.
    Shell {
        /// Run this agent instead of $SHELL (e.g. `claude`)
        #[arg(long)]
        agent: Option<String>,
        /// Deny network inside the sandbox
        #[arg(long)]
        offline: bool,
        /// Extra writable path (repeatable)
        #[arg(long = "allow")]
        allow: Vec<String>,
        /// One-shot command to run instead of an interactive shell
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// Snapshot the pristine install into games/<game>/vanilla-inventory.tsv —
    /// the untouched baseline `check --vanilla` (and the live test suite)
    /// verifies against. One-time setup per game.
    Inventory {
        /// Re-bless the CURRENT pristine as the baseline (overwrites)
        #[arg(long)]
        force: bool,
    },
    /// Remove every owned file from the instance; restore backups
    Undeploy {
        /// Remove owned files even if modified since realize
        #[arg(long)]
        force: bool,
    },
    /// Pipeline state per mod
    Status,
    /// Launch the game from the instance (reverse with `concierge-cli deactivate`)
    Launch {
        /// Don't launch — report launch-health (script-extender log + platform
        /// warnings) and pristine-safety instead
        #[arg(long)]
        check: bool,
    },
    /// Reverse a launch: restore the pristine at the Steam library path, the
    /// real launch stub, and the sandboxed load order. Idempotent.
    Deactivate,
    /// List a mod's FOMOD installer options (groups + options), marking the
    /// current `[mod.fomod]` selection. Use it to author/verify `select`.
    Fomod {
        /// Mod name (as in the manifest).
        name: String,
    },
    /// Point the game's canonical My Games at THIS profile's sandbox (isolated
    /// INIs/saves), or restore the real Documents with `--off`.
    Sandbox {
        /// Deactivate: drop the symlink and restore the parked real Documents.
        #[arg(long)]
        off: bool,
    },
    /// LOOT-powered load-order advice + masterlist warnings (Bethesda games)
    Sort {
        /// Apply the resolved order: rewrite plugins.txt in sorted order
        #[arg(long)]
        apply: bool,
    },
    /// Record-level conflict matrix across the load order (Bethesda games)
    Conflicts {
        /// Instead, report file-level (asset-path) conflicts across mods
        #[arg(long)]
        assets: bool,
    },
    /// Write the (v0: empty-shell) resolver plugin for this manifest
    Resolver,
    /// Auto-reconcile: realize, merge conflicting leveled lists into a real
    /// resolver plugin, deploy it, and activate it last (Bethesda games)
    Reconcile,
    /// Inspect a packed Bethesda archive (.ba2): list entries, or extract one
    Ba2 {
        /// Path to the .ba2 archive
        path: String,
        /// Extract this entry to stdout-reported bytes (writes next to the ba2)
        #[arg(long)]
        extract: Option<String>,
    },
    /// Rung 3 (Synthesize): the AI orchestrator over the deterministic core
    Ai {
        /// A natural-language goal for the autonomous loop (needs an API key)
        #[arg(long)]
        goal: Option<String>,
        /// Curate: search the catalog for this query
        #[arg(long)]
        catalog: Option<String>,
        /// Escalate: make --winner win this asset-conflict path (then verify)
        #[arg(long)]
        resolve: Option<String>,
        #[arg(long)]
        winner: Option<String>,
        /// Author: validate an AI-proposed acquisition pipeline for this mod
        /// name (the core runs + hashes it, `run` verb forbidden).
        #[arg(long)]
        propose_pipeline: Option<String>,
        /// The proposed pipeline steps as JSON, e.g.
        /// '[{"git":"https://github.com/u/r@v1"}]'
        #[arg(long)]
        steps: Option<String>,
    },
    /// Import a .wabbajack modlist you have (read-only) and report its mods,
    /// sources, and hashes — interop, not redistribution
    ImportModpack {
        /// Path to a .wabbajack file (or a raw modlist JSON)
        path: String,
        /// Also fetch + xxHash64-verify archives into the shared cache
        #[arg(long)]
        fetch: bool,
        /// When fetching, cap to N archives (the smallest of each source kind)
        #[arg(long, default_value = "2")]
        limit: usize,
        /// Write an evaluable Concierge manifest.toml here (a real test case:
        /// archives + sources + xxHash64 pins). Does not download anything.
        #[arg(long)]
        to_manifest: Option<String>,
    },
    /// Handle an `nxm://…` link (from the site's Mod Manager Download button):
    /// add the mod to the active profile, pinned to that file
    Nxm { url: String },
    /// Add a Modrinth mod (by slug or project id) to the active profile. Resolves
    /// the newest version matching the pack's game version + loader to a free CDN
    /// download (no key), then `download`/`realize` fetches it.
    Modrinth { project: String },
    /// Export this profile's pack (its `manifest.toml` recipe) to a shareable file
    ExportPack { dest: String },
    /// Import a shared pack into a new profile, keeping this machine's game paths
    ImportPack {
        /// Path to a shared pack manifest
        src: String,
        /// Name for the new profile (under the current game)
        profile: String,
    },
    /// Nexus helpers
    Nexus {
        #[command(subcommand)]
        cmd: NexusCmd,
    },
    /// Local metadata store (`ClickHouse`) for AI curation context
    Db {
        #[command(subcommand)]
        cmd: DbCmd,
    },
}

#[derive(Subcommand)]
enum DbCmd {
    /// Sync a game's mod catalog from the public Nexus GraphQL endpoint
    Sync { game: String },
    /// Run SQL against the local store (default output: `PrettyCompact`)
    Query {
        sql: String,
        #[arg(long, default_value = "PrettyCompact")]
        format: String,
    },
    /// Migrate a legacy `ClickHouse` catalog into the portable `SQLite` catalog
    /// (run once on a machine that has clickhouse; the result is cross-platform)
    Migrate,
}

#[derive(Subcommand)]
enum NexusCmd {
    /// Validate the API key
    Whoami,
    /// List a mod's downloadable files (to pin `nexus_file_id`)
    Files { mod_id: u64 },
    /// Auto-pick a mod's MAIN file and print the manifest fields to pin it
    Resolve { mod_id: u64 },
    /// Your tracked mods for this game — the wishlist to build a pack from
    Tracked,
    /// Check pinned mods for newer files on Nexus (update tracker)
    Updates,
}

/// Leaf directories under `root` that hold no entries — the empty-dir clutter
/// `doctor` reports. Recursive, best-effort (unreadable dirs are skipped).
fn empty_dirs(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let entries: Vec<_> = rd.flatten().collect();
        if entries.is_empty() {
            out.push(dir.to_path_buf());
            return;
        }
        for e in entries {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&e.path(), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

/// Unix-seconds timestamp (`ClickHouse` `best_effort` parses it).
#[cfg(feature = "clickhouse-migrate")]
fn chrono_free_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("{now}")
}

/// The (canonical location the game reads, this profile's sandbox target) pairs
/// to symlink for full isolation: `My Games` (INIs/saves) and — when configured —
/// `AppData` (`plugins.txt` / load order). The first pair is always `My Games`.
/// Errors when the profile hasn't opted into sandboxing (no `canonical_my_games`),
/// so non-sandbox profiles are simply unaffected.
fn sandbox_links(
    repo: &Repo,
    manifest: &Manifest,
) -> Result<Vec<(std::path::PathBuf, std::path::PathBuf)>> {
    let paths = &manifest.game.paths;
    let resolve = |p: &std::path::Path| {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            repo.profile.join(p)
        }
    };
    let canonical_mg = paths.get("canonical_my_games").ok_or_else(|| {
        Error::Other(
            "sandbox: set game.paths.canonical_my_games (the game's real My Games location) to enable".into(),
        )
    })?;
    let my_games = paths
        .get("my_games")
        .ok_or_else(|| Error::Other("sandbox: this profile has no my_games path".into()))?;
    let mut links = vec![(canonical_mg.clone(), resolve(my_games))];
    // load-order isolation: the game reads plugins.txt from AppData\Local\<Game>.
    // Symlink that dir per-profile so each profile keeps its own plugins.txt —
    // this is what stops a stale/shared load order from crashing a launch.
    if let Some(canonical_ad) = paths.get("canonical_appdata") {
        links.push((canonical_ad.clone(), repo.profile.join("sandbox/AppData")));
    }
    Ok(links)
}

/// Unconditional best-effort trace next to the executable, so `concierge`'s own
/// startup and shell path are visible even when it can't see `CONCIERGE_LOG_DIR`
/// (env not inherited by the child) or its stderr isn't reaching the terminal.
fn cli_trace(msg: &str) {
    use std::io::Write as _;
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
    else {
        return;
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("concierge-cli.log"))
    {
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

fn main() {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let ldir = std::env::var("CONCIERGE_LOG_DIR").unwrap_or_default();
    cli_trace(&format!(
        "start · args={:?} · cwd={cwd} · CONCIERGE_LOG_DIR={ldir:?}",
        std::env::args().collect::<Vec<_>>()
    ));
    // Wire every family/leaf adapter crate into core's resolver before anything
    // resolves a game kind.
    concierge_games::register();
    if let Err(e) = run() {
        cli_trace(&format!("run() returned error: {e}"));
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    cli_trace("run() returned Ok");
}

/// Nexus entries with no recorded "ok" verdict in state/audit.json (keyed by
/// mod id) — surfaced by eval/realize so unverified ids can't hide.
fn unaudited(repo: &Repo, manifest: &Manifest) -> usize {
    let audited: std::collections::BTreeSet<String> =
        std::fs::read_to_string(repo.state_dir().join("audit.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| {
                v.as_object().map(|o| {
                    o.iter()
                        .filter(|(_, e)| {
                            e.get("verdict").and_then(serde_json::Value::as_str) == Some("ok")
                        })
                        .map(|(k, _)| k.clone())
                        .collect()
                })
            })
            .unwrap_or_default();
    manifest
        .mods
        .iter()
        .filter_map(|m| m.nexus_mod_id)
        .filter(|id| !audited.contains(&id.to_string()))
        .count()
}

#[allow(clippy::too_many_lines)]
fn run() -> Result<()> {
    // Load persisted settings (download folder, concurrency, bandwidth) so the CLI
    // honours the same configuration as the GUI.
    let _ = concierge::settings::load();
    let cli = Cli::parse();
    // `init` runs in an empty dir — before repo discovery / plan eval.
    if let Cmd::Init { game, nix, dir } = &cli.cmd {
        let target = dir.clone().unwrap_or_else(|| ".".to_owned());
        return init::init(std::path::Path::new(&target), game, *nix);
    }
    // `adopt` CREATES a game dir — the workspace exists, the profile doesn't.
    if let Cmd::Adopt {
        name,
        pristine,
        game_version,
        mods_dir,
        mods_path,
        launch,
        nexus_domain,
        steam_app_id,
    } = &cli.cmd
    {
        let spec = concierge::profiles::AdoptSpec {
            pristine: std::path::PathBuf::from(pristine),
            version: game_version.clone(),
            mods_dir: mods_dir.clone(),
            mods_path: mods_path.as_ref().map(std::path::PathBuf::from),
            launch: launch.clone(),
            nexus_domain: nexus_domain.clone(),
            steam_app_id: *steam_app_id,
        };
        let ws = concierge::profiles::workspace()?;
        let profile = concierge::profiles::adopt_game(&ws, name, &spec)?;
        // Prove the scaffold is coherent NOW: load + eval the fresh manifest so
        // an agent gets immediate pass/fail instead of a deferred surprise.
        let manifest = Manifest::load(&profile)?;
        let plan = eval(&manifest)?;
        if !spec.pristine.is_dir() {
            eprintln!(
                "  ⚠ pristine does not exist (yet): {}",
                spec.pristine.display()
            );
        }
        println!(
            "  adopted   {} (kind {})",
            profile.display(),
            plan.game.kind
        );
        println!("  hash      {}", plan.hash()?);
        println!(
            "  next      add [[mod]] entries, then: CONCIERGE_REPO={} concierge realize",
            profile.display()
        );
        return Ok(());
    }
    // Converting a .wabbajack into a manifest CREATES a profile — no repo yet.
    if let Cmd::ImportModpack {
        path,
        to_manifest: Some(out),
        ..
    } = &cli.cmd
    {
        let p = std::path::Path::new(path);
        let is_json = p
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        let list = if is_json {
            let bytes = std::fs::read(p).ctx(p)?;
            concierge_modpack_import::ModList::from_modlist_json(&bytes)
        } else {
            concierge_modpack_import::ModList::from_modpack_archive(p)
        }
        .map_err(|e| Error::Other(e.to_string()))?;
        std::fs::write(out, list.to_manifest_toml()).ctx(std::path::Path::new(out))?;
        println!(
            "  manifest  wrote {} ({} mods, xxHash64-pinned) -> {out}",
            list.name,
            list.archives.len()
        );
        println!("  test case edit pristine/instance paths, then `concierge-cli eval` it");
        return Ok(());
    }
    // The headless agent view boots its own model — early-return like `init`.
    if let Cmd::Tui { script, live } = &cli.cmd {
        let code = headless::run(script.as_deref(), *live)?;
        std::process::exit(code);
    }
    let repo = Repo::discover()?;
    let manifest = Manifest::load(&repo.profile)?;
    let plan = eval(&manifest)?;

    match cli.cmd {
        Cmd::Eval => {
            std::fs::create_dir_all(repo.state_dir()).ctx(&repo.state_dir())?;
            let path = repo.plan_file();
            std::fs::write(&path, plan.canonical_json()?).ctx(&path)?;
            println!("  plan      {}", path.display());
            println!("  hash      {}", plan.hash()?);
            println!(
                "  mods      {} enabled, {}",
                plan.mods.len(),
                if plan.fully_pinned() {
                    "fully pinned"
                } else {
                    "UNPINNED entries present"
                }
            );
            let n = unaudited(&repo, &manifest);
            if n > 0 {
                println!(
                    "  audit     {n} Nexus entr{} unaudited — run `concierge-cli audit`",
                    if n == 1 { "y" } else { "ies" }
                );
            }
        }
        Cmd::Fetch => {
            let mut blocked = false;
            for (name, outcome) in fetch_all(&repo, &plan)? {
                match outcome {
                    FetchOutcome::Present(_) => println!("  ok        {name}"),
                    FetchOutcome::Stored(p) => println!("  stored    {}", p.display()),
                    FetchOutcome::NeedsPin { path, md5 } => {
                        println!("  stored    {}", path.display());
                        println!("  PIN       {name}: set md5 = \"{md5}\" in manifest.toml");
                    }
                    FetchOutcome::Blocked { instructions } => {
                        println!("  MISSING   {name}: {instructions}");
                        blocked = true;
                    }
                }
            }
            if blocked {
                return Err(Error::Other("some archives could not be fetched".into()));
            }
        }
        Cmd::Build => {
            for (name, outcome) in build_all(&repo, &plan)? {
                match outcome {
                    BuildOutcome::Present(_) => println!("  ok        {name}"),
                    BuildOutcome::Built(p) => println!("  built     {name} -> {}", p.display()),
                    BuildOutcome::Unpinned => {
                        println!("  skip      {name}: unpinned (run fetch, commit the pin)");
                    }
                }
            }
        }
        Cmd::Provision { all } => {
            let targets: Vec<std::path::PathBuf> = if all {
                let ws = concierge::profiles::workspace()?;
                let mut v = Vec::new();
                if let Ok(games) = std::fs::read_dir(ws.join("games")) {
                    for g in games.flatten() {
                        for p in concierge::profiles::list_profiles(&g.path()) {
                            v.push(p.dir);
                        }
                    }
                }
                v
            } else {
                vec![repo.profile]
            };
            for dir in targets {
                let kind = Manifest::load(&dir).map_or_else(|_| "game".to_owned(), |m| m.game.kind);
                let created = concierge::provision::provision_profile(&dir, &kind)?;
                println!(
                    "  {}  {} ({})",
                    if created.is_empty() {
                        "ok      "
                    } else {
                        "provision"
                    },
                    dir.display(),
                    if created.is_empty() {
                        "already agent-ready".to_owned()
                    } else {
                        created.join(", ")
                    }
                );
            }
        }
        Cmd::Audit => {
            let domain = plan
                .game
                .nexus_domain
                .clone()
                .unwrap_or_else(|| plan.game.kind.clone());
            let path = repo.catalog_path();
            if !path.exists() {
                return Err(Error::Other(format!(
                    "no catalog at {} — sync it first: `concierge-cli db sync {domain}`",
                    path.display()
                )));
            }
            let catalog = concierge_db::catalog::Catalog::open(&path)
                .map_err(|e| Error::Other(e.to_string()))?;
            // Disabled (parked) entries are audited too — they're the prime
            // suspects: unverified suggestions waiting for a verdict.
            let entries: Vec<(String, u64)> = manifest
                .mods
                .iter()
                .filter_map(|m| m.nexus_mod_id.map(|id| (m.name.clone(), id)))
                .collect();
            let report = concierge_db::audit::audit(&catalog, &domain, &entries)
                .map_err(|e| Error::Other(e.to_string()))?;
            let mut bad = 0usize;
            let mut record = serde_json::Map::new();
            for e in &report {
                use concierge_db::audit::Verdict;
                let (verdict, catalog_name) = match &e.verdict {
                    Verdict::Ok { catalog_name } => {
                        println!("  ok        {} (id {} = {catalog_name})", e.name, e.mod_id);
                        ("ok", catalog_name.clone())
                    }
                    Verdict::NameMismatch { catalog_name } => {
                        bad += 1;
                        println!(
                            "  MISMATCH  {} — id {} is actually '{catalog_name}'",
                            e.name, e.mod_id
                        );
                        ("name-mismatch", catalog_name.clone())
                    }
                    Verdict::UnknownId => {
                        bad += 1;
                        println!(
                            "  UNKNOWN   {} — id {} not in the {domain} catalog",
                            e.name, e.mod_id
                        );
                        ("unknown-id", String::new())
                    }
                };
                record.insert(
                    e.mod_id.to_string(),
                    serde_json::json!({
                        "name": e.name,
                        "verdict": verdict,
                        "catalog_name": catalog_name,
                    }),
                );
            }
            std::fs::create_dir_all(repo.state_dir()).ctx(&repo.state_dir())?;
            let audit_path = repo.state_dir().join("audit.json");
            std::fs::write(
                &audit_path,
                serde_json::to_string_pretty(&serde_json::Value::Object(record))?,
            )
            .ctx(&audit_path)?;
            if entries.is_empty() {
                println!("  no Nexus entries to audit");
            }
            if bad > 0 {
                return Err(Error::Other(format!(
                    "{bad} unverified id(s) — fix the manifest (search the catalog: \
                     `concierge-cli ai --catalog \"<name>\"`) and re-audit"
                )));
            }
        }
        Cmd::Lock => {
            concierge::profiles::set_locked(&repo.profile, true)?;
            println!(
                "  locked    {} (declaration read-only on disk)",
                repo.profile.display()
            );
        }
        Cmd::Unlock => {
            concierge::profiles::set_locked(&repo.profile, false)?;
            println!("  unlocked  {}", repo.profile.display());
        }
        Cmd::Realize { fresh, sort } => {
            if concierge::profiles::is_locked(&repo.profile) {
                return Err(Error::Other(
                    "profile is LOCKED (declaration read-only) — `concierge-cli unlock` first"
                        .into(),
                ));
            }
            let n = unaudited(&repo, &manifest);
            if n > 0 {
                eprintln!(
                    "  ⚠ {n} Nexus entr{} never audited — `concierge-cli audit` verifies the ids",
                    if n == 1 { "y" } else { "ies" }
                );
            }
            for (name, outcome) in fetch_all(&repo, &plan)? {
                if let FetchOutcome::Blocked { instructions } = outcome {
                    return Err(Error::Other(format!("{name}: {instructions}")));
                }
                if let FetchOutcome::NeedsPin { md5, .. } = outcome {
                    return Err(Error::Other(format!(
                        "{name}: unpinned — set md5 = \"{md5}\" in manifest.toml and re-run"
                    )));
                }
            }
            build_all(&repo, &plan)?;
            // Self-heal install layout (strip a versioned root into subdir,
            // activate detected plugins) — the same step the GUI's Apply runs —
            // then re-eval so the deploy sees it. A mod like JOURNEY realizes
            // without hand-editing the manifest.
            let relaid = concierge::realize::resolve_layouts(&repo, &plan)?;
            for line in &relaid {
                println!("  resolved  {line}");
            }
            let plan = if relaid.is_empty() {
                plan
            } else {
                eval(&Manifest::load(&repo.profile)?)?
            };
            let report = realize(&repo, &plan, fresh)?;
            if report.cloned_instance {
                println!("  cloned    pristine -> instance (CoW)");
            }
            println!(
                "  placed    {} files ({} owned, {} removed, {} backed up)",
                report.placed, report.total_owned, report.removed, report.backed_up
            );
            println!("  wrote     {} config file(s)", plan.configs.len());
            // Family diff-application, run against the materialized instance:
            // KOTOR merges each TSLPatcher changes.ini (2DA/TLK diff) into the
            // instance Override; other families are no-ops (their mods are
            // file-level overlays the generic deploy already placed).
            if let Ok(adapter) = concierge::game::adapter_for(&plan.game.kind) {
                let mods: Vec<(String, std::path::PathBuf)> = plan
                    .mods
                    .iter()
                    .filter_map(|m| m.md5.as_ref().map(|h| (m.name.clone(), repo.build_path(h))))
                    .collect();
                adapter.diff_apply(&concierge::game::DiffCtx {
                    instance_dir: std::path::Path::new(plan.game_dir()),
                    base_dir: std::path::Path::new(&plan.game.pristine),
                    mods,
                })?;
            }
            // Invariant guard: encode each game's crash-causing modding rules
            // (missing masters, plugin limits, unresolved dependencies, …) and
            // fail the build with a specific reason — the LOOT/MO2/SMAPI role.
            let (errors, warnings) = concierge_lint::partition(concierge_lint::validate(&plan)?);
            for w in &warnings {
                eprintln!("  ⚠ {} [{}]: {}", w.subject, w.rule, w.detail);
            }
            if !errors.is_empty() {
                eprintln!("  ✗ INVARIANT VIOLATIONS — the game would crash or the mod won't load:");
                for e in &errors {
                    eprintln!("      {} [{}]: {}", e.subject, e.rule, e.detail);
                }
                return Err(Error::Other(format!(
                    "{} invariant violation(s): fix the manifest/mods and re-realize",
                    errors.len()
                )));
            }
            println!("  realized  plan {}", plan.hash()?);
            // The correct final step: sort AFTER deploy wrote the activation
            // registry, so a good order isn't clobbered. Opt-in via --sort.
            if sort {
                if concierge::game::try_adapter(&plan.game.kind)
                    .and_then(concierge::game::GameAdapter::plugin_bases)
                    .is_some()
                {
                    let (path, order) =
                        concierge_pluginorder::loadorder::apply_order(&repo, &plan, None)?;
                    println!(
                        "  sorted    {} entries in LOOT order → {}",
                        order.len(),
                        path.display()
                    );
                } else {
                    println!("  sorted    (this game has no load order to sort)");
                }
            }
        }
        Cmd::Check { vanilla } => {
            let drift = check(&repo, &plan, vanilla)?;
            for d in &drift {
                match d {
                    Drift::Missing { key, owner } => {
                        println!("  MISSING   {key} (owned by {owner})");
                    }
                    Drift::Modified { key, owner } => {
                        println!("  MODIFIED  {key} (owned by {owner})");
                    }
                    Drift::PristineMissing { rel } => println!("  PRISTINE MISSING  {rel}"),
                    Drift::PristineChanged { rel } => println!("  PRISTINE CHANGED  {rel}"),
                    Drift::PlanMismatch { .. } => {
                        println!(
                            "  PLAN      realized state is from a different plan (re-run realize)"
                        );
                    }
                }
            }
            if drift.is_empty() {
                println!("  clean");
            } else {
                return Err(Error::Other(format!("{} drifted item(s)", drift.len())));
            }
        }
        Cmd::Preview { files } => {
            let deploys = concierge::realize::preview(&repo, &plan)?;
            let (mut total_files, mut total_active) = (0usize, 0usize);
            for d in &deploys {
                if d.unpinned {
                    println!("  {:<32} unpinned — fetch to preview", d.name);
                    continue;
                }
                let acts = if d.plugins.is_empty() {
                    String::new()
                } else {
                    format!("  → activates: {}", d.plugins.join(", "))
                };
                println!("  {:<32} {} file(s){acts}", d.name, d.files.len());
                total_files += d.files.len();
                total_active += d.plugins.len();
                if files {
                    for (root, rel) in &d.files {
                        println!("      {root}:{rel}");
                    }
                }
            }
            println!(
                "\n  {} mod(s) → {total_files} file(s), {total_active} activation entr(ies) — nothing deployed",
                deploys.len()
            );
        }
        Cmd::Plugins => {
            let is_plugin_game = concierge::game::try_adapter(&plan.game.kind)
                .and_then(concierge::game::GameAdapter::plugin_bases)
                .is_some();
            let lex = concierge::game::try_adapter(&plan.game.kind)
                .map(concierge::game::GameAdapter::lexicon);
            let order_word = lex.map_or("order", |l| l.order);
            if is_plugin_game {
                let rep = concierge_pluginorder::activation_report(&plan)?;
                println!("  {order_word} — {} active", rep.active.len());
                for (i, p) in rep.active.iter().enumerate() {
                    println!("    {:>3}  {p}", i + 1);
                }
                if rep.inert.is_empty() {
                    println!("\n  inert (deployed, not active): none");
                } else {
                    println!("\n  inert (deployed, not active): {}", rep.inert.len());
                    for p in &rep.inert {
                        println!("    · {p}");
                    }
                }
                if rep.missing_deps.is_empty() {
                    println!("  unresolved dependencies: none");
                } else {
                    println!("  unresolved dependencies:");
                    for m in &rep.missing_deps {
                        println!("    ✗ {} needs {}", m.plugin, m.missing.join(", "));
                    }
                }
            } else {
                // Games with no plugin registry: show the generic per-mod
                // activation entries the adapter would render.
                let mut n = 0;
                for m in &plan.mods {
                    if !m.plugins.is_empty() {
                        println!("  {:<32} {}", m.name, m.plugins.join(", "));
                        n += m.plugins.len();
                    }
                }
                println!(
                    "\n  {n} activation entr(ies) across {} mod(s)",
                    plan.mods.len()
                );
            }
        }
        Cmd::Doctor => {
            // Each check → (status, name, detail). PASS/WARN/FAIL: FAIL is a
            // guaranteed break (nonzero exit), WARN is harmless clutter. Built on
            // generic/adapter-dispatched backends — no game hardcoding here.
            const PASS: u8 = 0;
            const WARN: u8 = 1;
            const FAIL: u8 = 2;
            let mut results: Vec<(u8, &str, String)> = Vec::new();

            let unpinned: Vec<&str> = plan
                .mods
                .iter()
                .filter(|m| m.md5.is_none())
                .map(|m| m.name.as_str())
                .collect();
            results.push((
                if unpinned.is_empty() { PASS } else { FAIL },
                "pins",
                if unpinned.is_empty() {
                    format!("all {} mod(s) pinned", plan.mods.len())
                } else {
                    format!("{} unpinned: {}", unpinned.len(), unpinned.join(", "))
                },
            ));

            let (errs, warns) = concierge_lint::partition(concierge_lint::validate(&plan)?);
            results.push((
                if !errs.is_empty() {
                    FAIL
                } else if warns.is_empty() {
                    PASS
                } else {
                    WARN
                },
                "invariant lints",
                if errs.is_empty() {
                    format!(
                        "clean{}",
                        if warns.is_empty() {
                            String::new()
                        } else {
                            format!(" ({} warning(s))", warns.len())
                        }
                    )
                } else {
                    format!(
                        "{} violation(s): {}",
                        errs.len(),
                        errs.iter()
                            .map(|v| format!("{} {}", v.rule, v.subject))
                            .collect::<Vec<_>>()
                            .join("; ")
                    )
                },
            ));

            if concierge::game::try_adapter(&plan.game.kind)
                .and_then(concierge::game::GameAdapter::plugin_bases)
                .is_some()
            {
                if let Ok(rep) = concierge_pluginorder::activation_report(&plan) {
                    results.push((
                        if rep.inert.is_empty() { PASS } else { WARN },
                        "no inert plugins",
                        if rep.inert.is_empty() {
                            format!("{} active, 0 inert", rep.active.len())
                        } else {
                            format!(
                                "{} deployed but inactive: {}",
                                rep.inert.len(),
                                rep.inert.join(", ")
                            )
                        },
                    ));
                }
            }

            if let Some(inst) = &plan.game.instance {
                let inst_path = std::path::Path::new(inst);
                let pristine = std::path::Path::new(&plan.game.pristine);
                // Only mod-created empty dirs are clutter; a dir that also exists
                // in the pristine (vanilla `Mods/`, `Creations/`, …) is fine.
                let empties: Vec<_> = empty_dirs(inst_path)
                    .into_iter()
                    .filter(|d| {
                        d.strip_prefix(inst_path)
                            .ok()
                            .is_none_or(|rel| !pristine.join(rel).exists())
                    })
                    .collect();
                results.push((
                    if empties.is_empty() { PASS } else { WARN },
                    "no empty dirs",
                    if empties.is_empty() {
                        "instance clean".to_owned()
                    } else {
                        format!(
                            "{} empty dir(s), e.g. {}",
                            empties.len(),
                            empties
                                .first()
                                .map_or_else(String::new, |d| d.display().to_string())
                        )
                    },
                ));
            }

            let drift = check(&repo, &plan, false)?;
            results.push((
                if drift.is_empty() { PASS } else { FAIL },
                "no drift",
                if drift.is_empty() {
                    "deployed matches realized".to_owned()
                } else {
                    format!("{} drifted item(s) — re-realize", drift.len())
                },
            ));

            if repo.vanilla_inventory().exists() {
                let vdrift = check(&repo, &plan, true)?;
                let pd = vdrift
                    .iter()
                    .filter(|d| {
                        matches!(
                            d,
                            Drift::PristineMissing { .. } | Drift::PristineChanged { .. }
                        )
                    })
                    .count();
                results.push((
                    if pd == 0 { PASS } else { FAIL },
                    "pristine safe",
                    if pd == 0 {
                        "pristine matches the vanilla inventory".to_owned()
                    } else {
                        format!("{pd} pristine change(s) — the base install was modified!")
                    },
                ));
            } else {
                results.push((
                    PASS,
                    "pristine safe",
                    "no vanilla inventory (run `concierge-cli inventory` to enable this check)"
                        .to_owned(),
                ));
            }

            let (mut fails, mut warnings) = (0u32, 0u32);
            for (status, name, detail) in &results {
                let tag = match *status {
                    FAIL => "FAIL",
                    WARN => "WARN",
                    _ => "PASS",
                };
                println!("  [{tag}] {name:<18} {detail}");
                match *status {
                    FAIL => fails += 1,
                    WARN => warnings += 1,
                    _ => {}
                }
            }
            if fails == 0 {
                println!(
                    "\n  doctor: {}",
                    if warnings == 0 {
                        "all clear".to_owned()
                    } else {
                        format!("clear ({warnings} warning(s))")
                    }
                );
            } else {
                return Err(Error::Other(format!("doctor: {fails} check(s) failed")));
            }
        }
        Cmd::Inventory { force } => {
            let (path, n) = write_vanilla_inventory(&repo, &plan, force)?;
            println!("  inventory {} ({n} files hashed)", path.display());
        }
        Cmd::Shell {
            agent,
            offline,
            allow,
            cmd,
        } => {
            let extra: Vec<std::path::PathBuf> =
                allow.iter().map(std::path::PathBuf::from).collect();
            concierge::diag::event(
                "cli",
                "shell",
                &format!(
                    "start · kind={} · profile={} · agent={:?} · offline={offline} · cmd={cmd:?}",
                    plan.game.kind,
                    repo.profile.display(),
                    agent.as_deref(),
                ),
            );
            let mut c = concierge::shell::shell_command(
                &repo,
                &plan,
                agent.as_deref(),
                offline,
                &extra,
                &cmd,
            )
            .inspect_err(|e| {
                concierge::diag::event("cli", "error", &format!("shell_command failed: {e}"));
            })?;
            let argv: Vec<String> = std::iter::once(c.get_program())
                .chain(c.get_args())
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            concierge::diag::event("cli", "spawn", &format!("running {}", argv.join(" ")));
            cli_trace(&format!("shell: about to run: {}", argv.join(" ")));
            let status = c.status().map_err(|e| {
                concierge::diag::event(
                    "cli",
                    "error",
                    &format!("shell process failed to start: {e}"),
                );
                cli_trace(&format!("shell: process failed to start: {e}"));
                Error::Other(format!("sandboxed shell failed to start: {e}"))
            })?;
            concierge::diag::event(
                "cli",
                "exit",
                &format!("sandbox process exited: code={:?}", status.code()),
            );
            cli_trace(&format!(
                "shell: sandbox process exited code={:?}",
                status.code()
            ));
            std::process::exit(status.code().unwrap_or(1));
        }
        Cmd::Undeploy { force } => {
            let (removed, skipped) = undeploy(&repo, &plan, force)?;
            println!(
                "  undeployed {removed} files ({skipped} skipped); configs reset to no-mods state"
            );
        }
        Cmd::Status => {
            let state = Realized::load(&repo)?;
            let mut per_mod: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            for rec in state.files.values() {
                *per_mod.entry(rec.mod_name.as_str()).or_default() += 1;
            }
            println!("  game     {} (pristine, locked)", plan.game.version);
            match &plan.game.instance {
                Some(i) => println!(
                    "  instance {} @ {i}",
                    if std::path::Path::new(i).exists() {
                        "materialized"
                    } else {
                        "not materialized"
                    },
                ),
                None => println!("  instance in-place (deploys into the Steam install)"),
            }
            match &state.plan_hash {
                Some(h) if *h == plan.hash()? => println!("  state    in sync with current plan"),
                Some(_) => println!("  state    STALE (manifest changed since realize)"),
                None => println!("  state    nothing realized"),
            }
            for m in &plan.mods {
                let fetched = m
                    .md5
                    .as_ref()
                    .is_some_and(|h| repo.store_path(h, &m.file).exists());
                let built = m.md5.as_ref().is_some_and(|h| repo.build_path(h).exists());
                let deployed = per_mod.get(m.name.as_str()).copied().unwrap_or(0);
                println!(
                    "  {:<24} {:<10} {} {} {}",
                    m.name,
                    m.version,
                    if fetched { "fetched" } else { "-" },
                    if built { "built" } else { "-" },
                    if deployed > 0 {
                        format!("deployed({deployed})")
                    } else {
                        "-".into()
                    },
                );
            }
        }
        Cmd::Sandbox { off } => {
            let links = sandbox_links(&repo, &manifest)?;
            if off {
                for (canonical, _) in &links {
                    concierge::sandbox::deactivate(canonical)?;
                    println!("  sandbox   off — restored {}", canonical.display());
                }
            } else {
                for (canonical, sandbox) in &links {
                    concierge::sandbox::activate(canonical, sandbox)?;
                    println!(
                        "  sandbox   {} -> {}",
                        canonical.display(),
                        sandbox.display()
                    );
                }
                // My Games is always links[0]; wire the save redirect there.
                if let Some((_, my_games)) = links.first() {
                    if let Some(ini) =
                        concierge::sandbox::write_save_redirect(my_games, &manifest.game.kind)?
                    {
                        println!(
                            "  saves     SLocalSavePath=Saves\\ wired into {}",
                            ini.display()
                        );
                    }
                }
                println!("  isolated  own INIs + Saves + load order (nothing shared)");
            }
        }
        Cmd::Launch { check: true } => {
            // Diagnostics only — don't launch. The automated version of reading
            // f4se.log by hand + verifying the pristine.
            let my_games = manifest.game.paths.get("my_games").cloned();
            match my_games
                .as_deref()
                .zip(concierge::game::try_adapter(&plan.game.kind))
                .and_then(|(mg, a)| a.launch_health(&plan, mg))
            {
                Some(h) => {
                    println!("  script-extender: {} plugin(s) loaded", h.loaded.len());
                    if h.issues.is_empty() {
                        println!("  [PASS] no launch issues");
                    } else {
                        for i in &h.issues {
                            println!("  [WARN] {i}");
                        }
                    }
                }
                None => println!("  (no launch-health signal for this game)"),
            }
            // Pristine-safety: is the base install untouched?
            let pris = std::path::Path::new(&plan.game.pristine);
            if pris
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                println!("  [WARN] pristine is currently activated (a launch is live) — `concierge-cli deactivate` to restore it");
            } else if repo.vanilla_inventory().exists() {
                let pd = check(&repo, &plan, true)?
                    .iter()
                    .filter(|d| {
                        matches!(
                            d,
                            Drift::PristineMissing { .. } | Drift::PristineChanged { .. }
                        )
                    })
                    .count();
                println!(
                    "  [{}] pristine {}",
                    if pd == 0 { "PASS" } else { "FAIL" },
                    if pd == 0 {
                        "matches the vanilla inventory".to_owned()
                    } else {
                        format!("changed in {pd} place(s)!")
                    }
                );
            } else {
                println!("  [PASS] pristine in place (real dir; run `concierge-cli inventory` for a deep check)");
            }
        }
        Cmd::Launch { check: false } => {
            // If this profile is sandboxed, point the game at its My Games +
            // AppData (load order) first — so a stale shared plugins.txt can't
            // crash the launch.
            if let Ok(links) = sandbox_links(&repo, &manifest) {
                for (canonical, sandbox) in &links {
                    concierge::sandbox::activate(canonical, sandbox)?;
                }
                if let Some((_, my_games)) = links.first() {
                    concierge::sandbox::write_save_redirect(my_games, &manifest.game.kind)?;
                    println!(
                        "  sandbox   active: isolated My Games + load order -> {}",
                        my_games.display()
                    );
                }
            }
            let info = launch(&plan)?;
            if !info.steam_running {
                println!(
                    "  note: Steam is not running in the bottle — start it or the game will fail"
                );
            }
            println!(
                "  launched  {} ({:?}, from {})",
                info.exe,
                info.runtime,
                if info.from_instance {
                    "instance"
                } else {
                    "pristine"
                }
            );
        }
        Cmd::Deactivate => {
            // Reverse launch, outermost first: sandboxed load order, then the
            // Steam library symlink + launch stub (restores the pristine).
            if let Ok(links) = sandbox_links(&repo, &manifest) {
                for (canonical, _) in &links {
                    concierge::sandbox::deactivate(canonical)?;
                }
                if !links.is_empty() {
                    println!("  sandbox   restored: canonical My Games back in place");
                }
            }
            let info = deactivate(&plan)?;
            if info.stub_restored {
                println!("  stub      restored real launch stub in instance");
            }
            if info.instance_deactivated {
                println!(
                    "  pristine  restored at Steam library path ({})",
                    plan.game.pristine
                );
            }
        }
        Cmd::Fomod { name } => {
            let m =
                plan.mods.iter().find(|m| m.name == name).ok_or_else(|| {
                    Error::Other(format!("no mod named '{name}' in this profile"))
                })?;
            let Some(cfg) = concierge::realize::mod_fomod_config(&repo, m)? else {
                println!("  {name}: not a FOMOD (no fomod/ModuleConfig.xml), or not built yet");
                return Ok(());
            };
            // The effective selection: explicit picks merged over defaults.
            let picks: std::collections::HashSet<String> =
                m.fomod.clone().unwrap_or_default().into_iter().collect();
            let selected = cfg.selection_merged(&picks);
            let managed = m.fomod.is_some();
            println!("  {} — {}", name, cfg.module_name);
            println!(
                "  {}\n",
                if managed {
                    "[mod.fomod] managed (✓ = installed for the current select)"
                } else {
                    "not [mod.fomod]-managed — shown are the installer DEFAULTS (✓)"
                }
            );
            for step in &cfg.steps {
                println!("  {}", step.name);
                for g in &step.groups {
                    println!("    [{:?}] {}", g.kind, g.name);
                    for o in &g.options {
                        let mark = if selected.contains(&o.name) {
                            "✓"
                        } else {
                            " "
                        };
                        let files = if o.files.is_empty() {
                            String::new()
                        } else {
                            format!("  ({} file item(s))", o.files.len())
                        };
                        println!("      {mark} {}{files}", o.name);
                    }
                }
            }
        }
        Cmd::Db { cmd } => match cmd {
            DbCmd::Sync { game } => {
                // Sync writes the embedded SQLite catalog — no clickhouse needed,
                // so this works on Windows too.
                let mut cat = concierge_db::catalog::Catalog::open(&repo.catalog_path())
                    .map_err(|e| Error::Other(e.to_string()))?;
                // Accept a game KIND (skyrimse) or a Nexus domain
                // (skyrimspecialedition) — map a known kind to its domain so
                // `db sync skyrimse` doesn't silently hit the wrong endpoint.
                let domain = concierge::game::adapter_for(&game)
                    .ok()
                    .and_then(concierge::game::GameAdapter::nexus_domain)
                    .map_or_else(|| game.clone(), str::to_owned);
                let report = if let Some(md) = game.strip_prefix("modrinth:") {
                    concierge_db::sync::sync_modrinth(&mut cat, md, &mut |line| {
                        println!("{line}");
                    })
                } else {
                    concierge_db::sync::sync_game(&mut cat, &domain, &mut |line| {
                        println!("{line}");
                    })
                }
                .map_err(|e| Error::Other(e.to_string()))?;
                println!(
                    "  synced    {} rows in {} pages ({}; nexus reports {} mods)",
                    report.rows_synced,
                    report.pages,
                    if report.full_sweep {
                        "full sweep"
                    } else {
                        "incremental"
                    },
                    report.total_count
                );
            }
            #[cfg(not(feature = "clickhouse-migrate"))]
            DbCmd::Query { .. } | DbCmd::Migrate => {
                return Err(Error::Other(
                    "`db query` / `db migrate` are legacy ClickHouse commands; the catalog is \
                         now embedded SQLite. Rebuild with `--features clickhouse-migrate` (and \
                         clickhouse on PATH) only if you need to migrate an old store."
                        .into(),
                ));
            }
            #[cfg(feature = "clickhouse-migrate")]
            DbCmd::Query { sql, format } => {
                let db = concierge_db::ch::Db::open(&repo.db_dir())
                    .map_err(|e| Error::Other(e.to_string()))?;
                let out = db
                    .query(&sql, &format)
                    .map_err(|e| Error::Other(e.to_string()))?;
                print!("{out}");
            }
            #[cfg(feature = "clickhouse-migrate")]
            DbCmd::Migrate => {
                let db = concierge_db::ch::Db::open(&repo.db_dir())
                    .map_err(|e| Error::Other(e.to_string()))?;
                let path = repo.catalog_path();
                let mut cat = concierge_db::catalog::Catalog::open(&path)
                    .map_err(|e| Error::Other(e.to_string()))?;
                let out = db
                    .query(
                        "SELECT game_domain, mod_id, name, summary, author, version, category, \
                             endorsements, downloads, file_size, adult, \
                             toString(updated_at) AS updated_at FROM mods FINAL",
                        "JSONEachRow",
                    )
                    .map_err(|e| Error::Other(e.to_string()))?;
                let mut batch: Vec<concierge_db::catalog::Row> = Vec::new();
                let mut total = 0usize;
                let s = |v: &serde_json::Value, k: &str| {
                    v.get(k)
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_owned()
                };
                let n = |v: &serde_json::Value, k: &str| {
                    v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0)
                };
                for line in out.lines().filter(|l| !l.trim().is_empty()) {
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                        continue;
                    };
                    batch.push(concierge_db::catalog::Row {
                        game_domain: s(&v, "game_domain"),
                        mod_id: n(&v, "mod_id"),
                        name: s(&v, "name"),
                        summary: s(&v, "summary"),
                        author: s(&v, "author"),
                        version: s(&v, "version"),
                        category: s(&v, "category"),
                        endorsements: n(&v, "endorsements"),
                        downloads: n(&v, "downloads"),
                        file_size: n(&v, "file_size"),
                        adult: v
                            .get("adult")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                            || n(&v, "adult") != 0,
                        updated_at: s(&v, "updated_at"),
                    });
                    if batch.len() >= 5000 {
                        total += cat
                            .upsert(&batch)
                            .map_err(|e| Error::Other(e.to_string()))?;
                        batch.clear();
                    }
                }
                if !batch.is_empty() {
                    total += cat
                        .upsert(&batch)
                        .map_err(|e| Error::Other(e.to_string()))?;
                }
                println!("  migrated {total} mods -> {}", path.display());
            }
        },
        Cmd::Sort { apply } => {
            let report = concierge_pluginorder::loadorder::sort(&repo, &plan)?;
            if report.current == report.suggested {
                println!("  order     LOOT agrees with the current load order");
            } else {
                println!("  order     LOOT suggests:");
                for p in &report.suggested {
                    println!("            {p}");
                }
            }
            for name in &report.dirty {
                println!("  DIRTY     {name} (masterlist: needs cleaning)");
            }
            for (plugin, tags) in &report.tags {
                println!("  tags      {plugin}: {}", tags.join(", "));
            }
            if apply {
                let (path, order) =
                    concierge_pluginorder::loadorder::apply_order(&repo, &plan, None)?;
                println!(
                    "  applied   wrote {} entries to {} in resolved order",
                    order.len(),
                    path.display()
                );
            }
        }
        Cmd::Conflicts { assets: true } => {
            let conflicts = concierge_pluginorder::assets::asset_conflicts(&repo, &plan)?;
            let real: Vec<_> = conflicts.iter().filter(|c| !c.benign).collect();
            let benign = conflicts.len().saturating_sub(real.len());
            println!(
                "  assets    {} conflicting paths ({} real, {benign} benign/identical)",
                conflicts.len(),
                real.len()
            );
            for c in conflicts.iter().take(25) {
                println!(
                    "  {}  {} : {} -> winner {}",
                    if c.benign { "benign" } else { "REAL  " },
                    c.path,
                    c.providers.join(" + "),
                    c.winner
                );
            }
            if conflicts.len() > 25 {
                println!("  ... and {} more", conflicts.len() - 25);
            }
        }
        Cmd::Conflicts { assets: false } => {
            let (parsed, matrix) = concierge_pluginorder::conflict_matrix(&plan)?;
            println!("  parsed    {parsed} plugins");
            println!(
                "  scanned   {} records: {} overrides, {} conflicts ({} danger-class)",
                matrix.records_scanned,
                matrix.overrides,
                matrix.conflicts.len(),
                matrix.conflicts.iter().filter(|c| c.danger).count()
            );
            for c in matrix.conflicts.iter().take(15) {
                println!(
                    "  {}  {} {:06X} ({}): {} -> winner {}",
                    if c.danger { "DANGER" } else { "      " },
                    c.signature,
                    c.object_id,
                    c.origin,
                    c.carriers.join(" + "),
                    c.winner
                );
            }
            if matrix.conflicts.len() > 15 {
                println!("  ... and {} more", matrix.conflicts.len() - 15);
            }
            #[cfg(feature = "clickhouse-migrate")]
            {
                let db = concierge_db::ch::Db::open(&repo.db_dir())
                    .map_err(|e| Error::Other(e.to_string()))?;
                let rows =
                    concierge_pluginorder::conflict_rows(&plan, &matrix, &chrono_free_now())?;
                let n = db
                    .insert_json_rows("conflict_findings", &rows)
                    .map_err(|e| Error::Other(e.to_string()))?;
                println!("  recorded  {n} conflict rows in the metadata store");
            }
            #[cfg(not(feature = "clickhouse-migrate"))]
            println!("  (recording conflict rows to a metadata store needs --features clickhouse-migrate)");
        }
        Cmd::Resolver => {
            let (out, masters) = concierge_pluginorder::write_resolver(&repo, &plan)?;
            println!(
                "  wrote     {} (ESL-flagged, {masters} masters, self-validated)",
                out.display()
            );
        }
        Cmd::Reconcile => {
            // 1. deploy the mods so their plugins exist in the instance
            for (name, outcome) in fetch_all(&repo, &plan)? {
                if let FetchOutcome::Blocked { instructions } = outcome {
                    return Err(Error::Other(format!("{name}: {instructions}")));
                }
                if let FetchOutcome::NeedsPin { md5, .. } = outcome {
                    return Err(Error::Other(format!("{name}: unpinned (md5 = \"{md5}\")")));
                }
            }
            build_all(&repo, &plan)?;
            realize(&repo, &plan, false)?;
            // 2. merge conflicting leveled lists into a real resolver plugin
            let report = concierge_pluginorder::reconcile::reconcile(&repo, &plan)?;
            println!(
                "  scanned   {} plugins: {} conflicts ({} danger-class, left alone)",
                report.plugins, report.conflicts, report.danger
            );
            println!(
                "  merged    {} leveled lists + {} form-lists + {} UDR fixes -> {} resolver records",
                report.leveled_merged,
                report.formlist_merged,
                report.udr_fixed,
                report.resolver_records
            );
            if report.resolver_records == 0 {
                println!("  note      no mergeable conflicts — resolver is an empty shell");
            }
            // 3. deploy the resolver into the instance Data
            let resolver_name = "ConciergeResolver.esp";
            let data = std::path::PathBuf::from(plan.game_dir()).join("Data");
            std::fs::create_dir_all(&data).ctx(&data)?;
            let deployed = data.join(resolver_name);
            std::fs::copy(&report.resolver, &deployed).ctx(&deployed)?;
            // 4. apply the resolved load order to plugins.txt, resolver LAST
            let (_, order) =
                concierge_pluginorder::loadorder::apply_order(&repo, &plan, Some(resolver_name))?;
            println!(
                "  ordered   plugins.txt in resolved order ({} entries, resolver last)",
                order.len()
            );
            println!(
                "  deployed  {resolver_name} ({} masters)",
                report.masters.len()
            );
        }
        Cmd::Ba2 { path, extract } => {
            let bytes = std::fs::read(&path).ctx(std::path::Path::new(&path))?;
            let archive = concierge_ba2::Archive::parse(&bytes)
                .map_err(|e| Error::Other(format!("{path}: {e}")))?;
            println!(
                "  archive   {:?}, {} entries",
                archive.kind(),
                archive.len()
            );
            if let Some(name) = extract {
                let data = archive
                    .extract(&name)
                    .map_err(|e| Error::Other(e.to_string()))?;
                let out = std::path::Path::new(&path)
                    .with_file_name(name.rsplit(['/', '\\']).next().unwrap_or("extracted"));
                std::fs::write(&out, &data).ctx(&out)?;
                println!("  extracted {} bytes -> {}", data.len(), out.display());
            } else {
                for entry in archive.entries().iter().take(20) {
                    println!("            {}", entry.name);
                }
                if archive.len() > 20 {
                    println!("            ... and {} more", archive.len() - 20);
                }
            }
        }
        Cmd::Ai {
            goal,
            catalog,
            resolve,
            winner,
            propose_pipeline,
            steps,
        } => {
            if let Some(name) = propose_pipeline {
                let steps_json: serde_json::Value =
                    serde_json::from_str(steps.as_deref().unwrap_or("[]"))
                        .map_err(|e| Error::Other(format!("--steps must be JSON: {e}")))?;
                println!("  AI PROPOSES a pipeline for '{name}' (not on Nexus); core validates:");
                let v = concierge_ai::tools::validate_pipeline(&steps_json, &name, &repo.store())
                    .map_err(|e| Error::Other(e.to_string()))?;
                println!("    md5 (pin)   {}", v.md5);
                println!("    files       {}", v.file_count);
                println!("    usable      {}", v.usable);
                for s in v.sample.iter().take(8) {
                    println!("      {s}");
                }
                if v.usable {
                    println!(
                        "    -> pin md5 = \"{}\" into the mod's manifest entry to make it reproducible",
                        v.md5
                    );
                }
            } else if let Some(query) = catalog {
                let game = plan
                    .game
                    .nexus_domain
                    .clone()
                    .unwrap_or_else(|| plan.game.kind.clone());
                let filter = concierge::manifest::Manifest::load(&repo.profile)
                    .ok()
                    .map_or_else(concierge_ai::tools::CatalogFilter::default, |m| {
                        concierge_ai::tools::CatalogFilter::from_curate(&m.curate)
                    });
                let hits = concierge_ai::tools::catalog_search(&repo, &game, &query, 8, &filter)
                    .map_err(|e| Error::Other(e.to_string()))?;
                println!("  catalog   '{query}' in {game}:");
                for h in &hits {
                    println!(
                        "            {} ({} endorse, {} KB)  mod {}",
                        h.name, h.endorsements, h.kb, h.mod_id
                    );
                }
            } else if let (Some(path), Some(win)) = (resolve, winner) {
                let decision = concierge_ai::AssetDecision {
                    path,
                    winner_mod: win,
                    reason: "model-chosen winner".to_owned(),
                };
                let report =
                    concierge_ai::apply_decision(&repo, &plan, std::slice::from_ref(&decision))
                        .map_err(|e| Error::Other(e.to_string()))?;
                println!("  RUNG-BY-RUNG REPORT");
                println!("    plugins                 {}", report.plugins);
                println!(
                    "    rung2 mergeable records {}",
                    report.reconcilable_record_conflicts
                );
                println!("    rung2 refused (danger)  {}", report.danger_class);
                println!("    asset conflicts         {}", report.asset_conflicts);
                for r in &report.synthesized {
                    println!(
                        "    rung3 SYNTHESIZED  {} -> {} (hash {}, verified={})",
                        r.path, r.chosen_winner, r.winner_hash, r.verified
                    );
                }
                println!(
                    "    oracle (vanilla+plan)   {}",
                    if report.oracle_clean {
                        "clean"
                    } else {
                        "DRIFT"
                    }
                );
            } else if let Some(g) = goal {
                match concierge_ai::run_agent(&repo, &plan, &g, "claude-sonnet-5") {
                    Ok(r) => println!("  agent done: {r:?}"),
                    Err(concierge_ai::Error::NoKey) => {
                        println!(
                            "  autonomous loop needs a key at ~/.config/concierge/anthropic-api-key"
                        );
                        println!(
                            "  (tools + verification run without it; use --catalog / --resolve)"
                        );
                    }
                    Err(e) => return Err(Error::Other(e.to_string())),
                }
            } else {
                let l = concierge_ai::tools::conflict_landscape(&repo, &plan)
                    .map_err(|e| Error::Other(e.to_string()))?;
                println!(
                    "  landscape {} plugins, {} record conflicts ({} danger, {} mergeable)",
                    l.plugins, l.record_conflicts, l.danger_class, l.mergeable_left
                );
                println!(
                    "  assets    {} conflicts the deterministic rung won't merge:",
                    l.asset_conflicts.len()
                );
                for c in l.asset_conflicts.iter().take(10) {
                    println!(
                        "            {} : {} (load-order winner {})",
                        c.path,
                        c.providers.join(" + "),
                        c.winner_by_load_order
                    );
                }
            }
        }
        Cmd::ImportModpack {
            path,
            fetch,
            limit,
            to_manifest: _, // handled before repo discovery
        } => {
            let p = std::path::Path::new(&path);
            let is_json = p
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"));
            let list = if is_json {
                let bytes = std::fs::read(p).ctx(p)?;
                concierge_modpack_import::ModList::from_modlist_json(&bytes)
            } else {
                concierge_modpack_import::ModList::from_modpack_archive(p)
            }
            .map_err(|e| Error::Other(e.to_string()))?;
            let nexus = list.nexus_mods();
            #[allow(clippy::cast_precision_loss)]
            #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
            let gb = list.total_size() as f64 / 1e9;
            println!(
                "  modlist   {} by {} ({})",
                list.name, list.author, list.game
            );
            println!(
                "  archives  {} total, {gb:.1} GB, {} from Nexus",
                list.archives.len(),
                nexus.len()
            );
            for kind in &[
                "Http",
                "Mega",
                "GoogleDrive",
                "WabbajackCDN",
                "GameFile",
                "Manual",
            ] {
                let n = list
                    .archives
                    .iter()
                    .filter(|a| format!("{:?}", a.source).contains(kind))
                    .count();
                if n > 0 {
                    println!("            {n} from {kind}");
                }
            }
            println!(
                "  note      Wabbajack hashes are xxHash64 (base64), not the md5 Concierge pins"
            );
            println!("  nexus mods (map to [[mod]] entries):");
            for a in nexus.iter().take(12) {
                if let concierge_modpack_import::Source::Nexus {
                    mod_id, file_id, ..
                } = &a.source
                {
                    println!("            {mod_id}/{file_id}  {}", a.name);
                }
            }
            if nexus.len() > 12 {
                println!("            ... and {} more", nexus.len() - 12);
            }
            if fetch {
                use concierge::store::{fetch_verified, Remote, VerifiedFetch};
                // fetch + verify the smallest archive of EACH fetchable source
                // kind (so both the Http and Nexus paths are exercised with the
                // least download), capped at `limit`.
                let mut smallest: std::collections::BTreeMap<
                    &str,
                    &concierge_modpack_import::Archive,
                > = std::collections::BTreeMap::new();
                for a in &list.archives {
                    let kind = match &a.source {
                        concierge_modpack_import::Source::Nexus { .. } => "Nexus",
                        concierge_modpack_import::Source::Http { .. } => "Http",
                        concierge_modpack_import::Source::Other { .. } => continue,
                    };
                    let smaller = smallest.get(kind).is_none_or(|e| a.size < e.size);
                    if smaller {
                        smallest.insert(kind, a);
                    }
                }
                println!("  fetch     verifying the smallest of each source kind against their xxHash64:");
                for a in smallest.into_values().take(limit) {
                    let remote = match &a.source {
                        concierge_modpack_import::Source::Nexus {
                            game,
                            mod_id,
                            file_id,
                        } => Remote::Nexus {
                            game_domain: game.to_lowercase(),
                            mod_id: *mod_id,
                            file_id: *file_id,
                        },
                        concierge_modpack_import::Source::Http { url } => {
                            Remote::Http { url: url.clone() }
                        }
                        concierge_modpack_import::Source::Other { kind } => {
                            Remote::Unsupported { kind: kind.clone() }
                        }
                    };
                    match fetch_verified(&repo.store(), &a.name, &remote, &a.hash)? {
                        VerifiedFetch::Cached(_) => {
                            println!("  CACHED    {} (xxHash64 verified, no download)", a.name);
                        }
                        VerifiedFetch::Fetched(_) => {
                            println!(
                                "  VERIFIED  {} ({} bytes, xxHash64 matched)",
                                a.name, a.size
                            );
                        }
                        VerifiedFetch::HashMismatch { expected, actual } => {
                            println!("  MISMATCH  {}: expected {expected}, got {actual}", a.name);
                        }
                        VerifiedFetch::NoKey => {
                            println!("  NOKEY     {}: Nexus source needs a premium key", a.name);
                        }
                        VerifiedFetch::Unsupported { kind } => {
                            println!(
                                "  MANUAL    {}: {kind} not auto-fetched (get it yourself)",
                                a.name
                            );
                        }
                    }
                }
            }
        }
        Cmd::Nxm { url } => {
            let n = nexus::parse_nxm(&url)
                .ok_or_else(|| Error::Other(format!("not a valid nxm:// url: {url}")))?;
            let name = concierge_ai::tools::catalog_names(&repo, &n.domain, &[n.mod_id])
                .ok()
                .and_then(|v| v.into_iter().next().map(|(_, mn)| mn))
                .unwrap_or_else(|| format!("nexus-mod-{}", n.mod_id));
            // If the link carries the click token, download the file with it now
            // (free-user, one-click, TOS-sanctioned) and pin the exact hash.
            let (mut md5, mut file) = (String::new(), String::new());
            if let (Some(k), Some(exp), Ok(api)) = (&n.key, &n.expires, nexus::api_key()) {
                match concierge::store::acquire_nxm(
                    &repo, &n.domain, n.mod_id, n.file_id, &api, k, exp,
                ) {
                    Ok((got, fname)) => {
                        println!("  downloaded + pinned '{fname}' (md5 {got})");
                        md5 = got;
                        file = fname;
                    }
                    Err(e) => println!("  token download failed ({e}); adding unpinned"),
                }
            }
            let path = repo.profile.join("manifest.toml");
            let text = std::fs::read_to_string(&path)
                .map_err(|e| Error::Other(format!("read manifest: {e}")))?;
            let entry = concierge::manifest_edit::NewMod {
                name: name.clone(),
                version: "1".to_owned(),
                source: concierge::manifest_edit::NewSource::Nexus {
                    mod_id: u32::try_from(n.mod_id).unwrap_or(0),
                    file_id: u32::try_from(n.file_id).unwrap_or(0),
                },
                md5,
                file,
                install_root: "data".to_owned(),
                plugins: Vec::new(),
            };
            let new = concierge::manifest_edit::add_mod(&text, &entry)
                .map_err(|e| Error::Other(e.to_string()))?;
            std::fs::write(&path, new).map_err(|e| Error::Other(format!("write: {e}")))?;
            println!("  added '{name}' (nexus {} file {})", n.mod_id, n.file_id);
        }
        Cmd::Modrinth { project } => {
            // Resolve against the pack's declared Minecraft version + loader so
            // we pick the right build; unset filters fall back to the newest.
            let manifest = Manifest::load(&repo.profile)?;
            let gv = manifest.compat.game_version.as_deref();
            let loader = manifest.compat.loader.as_deref();
            let r = concierge_db::modrinth::resolve(&project, gv, loader)
                .map_err(|e| Error::Other(e.to_string()))?;
            println!(
                "  resolved {project} {} → {} (free: {})",
                r.version_number, r.filename, r.url
            );
            let path = repo.profile.join("manifest.toml");
            let text = std::fs::read_to_string(&path)
                .map_err(|e| Error::Other(format!("read manifest: {e}")))?;
            let entry = concierge::manifest_edit::NewMod {
                name: project.clone(),
                version: r.version_number,
                // A plain HTTPS CDN URL — the existing url-source path downloads
                // it for free and pins the computed md5 on first fetch.
                source: concierge::manifest_edit::NewSource::Url(r.url),
                md5: String::new(),
                file: r.filename,
                install_root: "data".to_owned(),
                plugins: Vec::new(),
            };
            let new = concierge::manifest_edit::add_mod(&text, &entry)
                .map_err(|e| Error::Other(e.to_string()))?;
            std::fs::write(&path, new).map_err(|e| Error::Other(format!("write: {e}")))?;
            println!("  added '{project}' from Modrinth — run `download` to fetch it (free).");
        }
        Cmd::ExportPack { dest } => {
            let src = repo.profile.join("manifest.toml");
            std::fs::copy(&src, &dest).map_err(|e| Error::Other(format!("copy: {e}")))?;
            println!("  exported pack recipe -> {dest}");
            println!("  share it; the recipient runs `concierge-cli import-pack {dest} <name>`");
        }
        Cmd::ImportPack { src, profile } => {
            let shared = std::fs::read_to_string(&src)
                .map_err(|e| Error::Other(format!("read {src}: {e}")))?;
            let local = std::fs::read_to_string(repo.profile.join("manifest.toml"))
                .map_err(|e| Error::Other(format!("read local manifest: {e}")))?;
            let merged = concierge::manifest_edit::swap_game_block(&shared, &local)
                .map_err(|e| Error::Other(e.to_string()))?;
            let game_dir = repo
                .profile
                .parent()
                .and_then(std::path::Path::parent)
                .ok_or_else(|| Error::Other("cannot locate game dir".into()))?;
            let new_dir = concierge::profiles::create_profile(game_dir, &profile, None)
                .map_err(|e| Error::Other(e.to_string()))?;
            std::fs::write(new_dir.join("manifest.toml"), merged)
                .map_err(|e| Error::Other(format!("write: {e}")))?;
            println!("  imported pack into profile '{profile}' (using this machine's game paths)");
            println!("  -> select it, then Download + Realize");
        }
        Cmd::Nexus { cmd } => match cmd {
            NexusCmd::Whoami => {
                let key = nexus::api_key()?;
                let u = nexus::validate(&key)?;
                println!("  {} premium={}", u.name, u.is_premium);
            }
            NexusCmd::Files { mod_id } => {
                let key = nexus::api_key()?;
                let domain = plan
                    .game
                    .nexus_domain
                    .ok_or_else(|| Error::Other("this game has no Nexus domain".into()))?;
                for f in nexus::files(&key, &domain, mod_id)? {
                    let cat = f.category_name.unwrap_or_default();
                    if cat.is_empty() || cat == "OLD_VERSION" || cat == "ARCHIVED" {
                        continue;
                    }
                    println!(
                        "  file_id={:<10} {:<12} v{:<10} {}",
                        f.file_id,
                        cat,
                        f.version.unwrap_or_else(|| "?".into()),
                        f.file_name
                    );
                }
            }
            NexusCmd::Resolve { mod_id } => {
                let key = nexus::api_key()?;
                let domain = plan
                    .game
                    .nexus_domain
                    .ok_or_else(|| Error::Other("this game has no Nexus domain".into()))?;
                let f = nexus::main_file(&key, &domain, mod_id)?;
                println!("  resolved main file for mod {mod_id}:");
                println!("    nexus_file_id = {}", f.file_id);
                println!("    file = \"{}\"", f.file_name);
                println!(
                    "  -> set those on the [[mod]] entry, then `concierge-cli fetch` downloads + pins md5"
                );
            }
            NexusCmd::Tracked => {
                let key = nexus::api_key()?;
                let domain = plan
                    .game
                    .nexus_domain
                    .ok_or_else(|| Error::Other("this game has no Nexus domain".into()))?;
                let ids: Vec<u64> = nexus::tracked_mods(&key)?
                    .into_iter()
                    .filter(|t| t.domain_name == domain)
                    .map(|t| t.mod_id)
                    .collect();
                let by_id: std::collections::HashMap<u64, String> =
                    concierge_ai::tools::catalog_names(&repo, &domain, &ids)
                        .map_err(|e| Error::Other(e.to_string()))?
                        .into_iter()
                        .collect();
                println!("  tracked ({}) in {domain} — your wishlist:", ids.len());
                for id in &ids {
                    let label = by_id.get(id).map_or("(not in catalog)", String::as_str);
                    println!("    mod {id:<8} {label}");
                }
                println!(
                    "  -> add the ones you want as [[mod]] (nexus_mod_id = <id>), then `nexus resolve` + `fetch`"
                );
            }
            NexusCmd::Updates => {
                let key = nexus::api_key()?;
                let domain = plan
                    .game
                    .nexus_domain
                    .ok_or_else(|| Error::Other("this game has no Nexus domain".into()))?;
                let m = concierge::manifest::Manifest::load(&repo.profile)
                    .map_err(|e| Error::Other(e.to_string()))?;
                let mut updates = 0;
                for md in &m.mods {
                    let (Some(mod_id), Some(file_id)) = (md.nexus_mod_id, md.nexus_file_id) else {
                        continue;
                    };
                    match nexus::main_file(&key, &domain, mod_id) {
                        Ok(f) if f.file_id != file_id => {
                            updates += 1;
                            let v = f.version.unwrap_or_else(|| "?".into());
                            println!(
                                "  {} : update available — file {file_id} -> {} (v{v})",
                                md.name, f.file_id
                            );
                        }
                        Ok(_) => {}
                        Err(e) => println!("  {} : check failed ({e})", md.name),
                    }
                }
                if updates == 0 {
                    println!("  all pinned Nexus mods are current");
                } else {
                    println!(
                        "  {updates} update(s) — set the new nexus_file_id + clear md5, then `fetch`"
                    );
                }
            }
        },
        // handled before repo discovery
        // all handled before repo discovery (init/adopt scaffold; tui boots its
        // own headless model and exits)
        Cmd::Init { .. } | Cmd::Adopt { .. } | Cmd::Tui { .. } => {}
    }
    Ok(())
}
