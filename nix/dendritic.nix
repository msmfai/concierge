# concierge dendritic module system — a lean, pure module tree.
#
# The dendritic pattern: a modpack is a *tree of small single-concern modules*
# that compose only through this merge (never manual cross-imports). Each module
# is an attrset fragment `{ mods = [...]; ini = {...}; }` or a function
# `self: fragment` (an overlay that sees the merged result). We deliberately
# avoid nixpkgs `lib` so this also runs on a lean evaluator (tvix) — pure
# builtins only.
#
# Metamorphic laws are checked BY THE INTERPRETER: `mkDendritic` `assert`s them,
# so a tree that violates order-independence, identity, or uniqueness fails to
# evaluate. The compiler is the property checker.
let
  inherit (builtins) foldl';
  inherit (builtins) sort;
  inherit (builtins) isFunction;

  reverse = xs: foldl' (acc: x: [x] ++ acc) [] xs;
  sortStr = sort (a: b: a < b);

  emptyFragment = {
    mods = [];
    ini = {};
  };

  # concat mods (order = load order, semantic) + shallow-merge ini sections
  mergeFragment = a: b: {
    mods = (a.mods or []) ++ (b.mods or []);
    ini = (a.ini or {}) // (b.ini or {});
  };

  # a module is a fragment or (self: fragment); overlays see the final merge
  resolve = self: m:
    if isFunction m
    then m self
    else m;

  evalTree = {
    game,
    modules ? [],
    ini ? {},
  }: let
    final =
      foldl' (acc: m: mergeFragment acc (resolve final m))
      (emptyFragment // {inherit ini;})
      modules;
  in
    {inherit game;}
    // (
      if final.ini != {}
      then {inherit (final) ini;}
      else {}
    )
    // (
      if final.mods != []
      then {mod = final.mods;}
      else {}
    );

  namesOf = mp: map (m: m.name) (mp.mod or []);
  setOf = mp: sortStr (namesOf mp);
  unique = xs:
    foldl' (acc: x:
      if builtins.elem x acc
      then acc
      else acc ++ [x]) []
    xs;

  # --- metamorphic + domain laws the evaluator enforces ---
  laws = {
    game,
    modules,
  }: let
    base = evalTree {inherit game modules;};
    rev = evalTree {
      inherit game;
      modules = reverse modules;
    };
    withEmpty = evalTree {
      inherit game;
      modules = modules ++ [emptyFragment];
    };
    names = namesOf base;
  in {
    # membership is order-independent (load ORDER may differ; the SET can't)
    orderIndependentSet = setOf base == setOf rev;
    # an empty module changes nothing
    identity = base == withEmpty;
    # no two modules claim the same mod name (a real conflict, not a merge)
    uniqueNames = builtins.length names == builtins.length (unique names);
  };
in rec {
  inherit evalTree emptyFragment laws;

  # build a modpack AND assert its laws — eval fails if any law is violated.
  mkDendritic = {
    game,
    modules ? [],
  }: let
    l = laws {inherit game modules;};
  in
    assert l.orderIndependentSet;
    assert l.identity;
    assert l.uniqueNames;
      evalTree {inherit game modules;};
}
