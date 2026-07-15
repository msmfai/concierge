# UX naming: plain words over system jargon

## The principle
**Nielsen's Heuristic #2 — Match Between System and the Real World:** *"The design
should speak the users' language, with words, phrases, and concepts familiar to
the user, rather than internal jargon. Follow real-world conventions."* Reinforced
by **#4 Consistency and Standards** (match what mod managers already say) and
**#6 Recognition rather than Recall** (a label should explain itself, not require
learning our mental model — Cooper: *don't make the user learn the implementation*).

Concierge's model is Nix-inspired, and the labels leaked the Nix vocabulary
(*realize*, *declaration*, *realised*, *generations*, *reconcile*). Those are
precise for us and opaque to a modder. The mental model we want a user to hold is
simple:

> **You design a Setup → you Apply it → you see what's Installed.**

So we lead with plain verbs and nouns, and keep the precise term in the tooltip
for power users (progressive disclosure).

## The rename map (user-facing labels only; code identifiers unchanged)

| Was (jargon) | Now (plain) | Why |
|---|---|---|
| Realize / Rebuild | **Apply** | "apply your setup," a universal verb, not the Nix term |
| Declaration (tab) | **Setup** | the mods you want — your plan |
| Realised (tab) | **Installed** | what's actually deployed to the game |
| Undeploy | **Uninstall** | the familiar opposite of install |
| Launch | **Play** | the universal word for starting a game |
| Reconcile | **Merge conflicts** | names the action, not the internal step |
| Sort (LOOT) | **Sort order** | lead with the action; "LOOT" stays in the tooltip |
| Resolve deps | **Requirements** | "what a mod needs," not "deps" |
| Suggest patches | **Find patches** | a plain verb |
| Verify | **Verify** (kept) | common word; tooltip drops "drift" |
| Conflicts | **Conflicts** (kept) | already plain |
| generation(s) | **version(s)** / **backup** | a saved snapshot of your setup / of your saves |
| Rollback | **Restore** | go back to a saved version |
| ApplyDiff / Pending changes | **Preview changes** | see it before you do it |
| lockfile / profile lock | **Pinned versions** | the exact versions locked in |
| mutable / immutable | **Edit / Locked** (kept) | already plain |

Tooltips carry the one-line "what it does" and, where useful, the underlying
technical term in parentheses so docs and power users stay connected.

## Game-specific vocabulary overrides the generic labels

"Match the real world" is *per domain* — each game's community has its own words,
and those beat our generic ones. So `GameAdapter` carries a **`Lexicon`** (a small
set of overridable terms with generic defaults). The active game's lexicon drives
the labels; a game overrides only what differs.

| Term | Generic default | Bethesda (`BETHESDA_LEXICON`) |
|---|---|---|
| `order` | "order" | **"load order"** |
| `sort_action` | "Sort order" | **"Sort load order"** |
| `plugins` | "files" | **"plugins"** |

So on a Fallout 4 / Skyrim profile the button reads **"⇅ Sort load order"**, the
section is **"… load order (N plugins)"**, and the preview says **"load order will
change"** — the words a Bethesda modder already uses. A game with no override just
gets the generic words. New games add their own `Lexicon` in `game.rs` and the UI
follows automatically (no GUI edits).
