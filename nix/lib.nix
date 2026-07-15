# concierge.lib — the pure Nix DSL for modpacks.
#
# Evaluates to the exact `Manifest` attrset the Rust core deserializes. Pure:
# no derivations, no store, no impurity — so it runs anywhere the Nix *language*
# runs, and acquisition stays Concierge's own hash-pinned fetchers. Both this
# and manifest.toml produce a byte-identical plan (the differential oracle).
rec {
  # --- source helpers: each returns the acquisition fields for a mod node ---

  # a Nexus mod (premium auto-download / free manual + hash-detect)
  nexus = {
    mod,
    file,
    md5 ? "",
    xxhash ? "",
  }:
    {
      nexus_mod_id = mod;
      nexus_file_id = file;
    }
    // (
      if md5 != ""
      then {inherit md5;}
      else {}
    )
    // (
      if xxhash != ""
      then {inherit xxhash;}
      else {}
    );

  # a direct HTTP archive
  http = {
    url,
    md5 ? "",
  }:
    {inherit url;}
    // (
      if md5 != ""
      then {inherit md5;}
      else {}
    );

  # github "owner/repo@ref"  ->  a git-clone pipeline (deterministic git-archive)
  github = spec: {pipeline = [{git = "https://github.com/" + spec;}];};

  # an explicit acquisition pipeline: [ {git=…} {extract=true} {pick="Data"} … ]
  pipeline = steps: {pipeline = steps;};

  # the optional Nix-FOD tier: a raw fetcher expression Concierge `nix`-realizes
  nixFetch = expr: {nix = expr;};

  # fetchGit sugar (rev-pinned, Nix-verified):
  #   nixGit { url = "https://github.com/u/r"; rev = "<sha>"; ref = "refs/tags/v1"; }
  nixGit = {
    url,
    rev,
    ref ? null,
  }: {
    nix =
      "builtins.fetchGit { url = \"${url}\"; rev = \"${rev}\";"
      + (
        if ref != null
        then " ref = \"${ref}\";"
        else ""
      )
      + " }";
  };

  # --- a mod node: placement + a source helper, merged, only-set-fields ---
  mkMod = {
    name,
    version,
    source,
    install_root ? null,
    subdir ? null,
    plugins ? [],
    enabled ? true,
    md5 ? "",
  }:
    {inherit name version;}
    // (
      if enabled
      then {}
      else {inherit enabled;}
    )
    // (
      if install_root != null
      then {inherit install_root;}
      else {}
    )
    // (
      if subdir != null
      then {inherit subdir;}
      else {}
    )
    // (
      if plugins != []
      then {inherit plugins;}
      else {}
    )
    // source
    // (
      if md5 != ""
      then {inherit md5;}
      else {}
    );

  # --- the modpack: game shape + mods -> the Manifest attrset ---
  mkModpack = {
    game,
    mods ? [],
    ini ? {},
  }:
    {inherit game;}
    // (
      if ini != {}
      then {inherit ini;}
      else {}
    )
    // (
      if mods != []
      then {mod = mods;}
      else {}
    );
}
