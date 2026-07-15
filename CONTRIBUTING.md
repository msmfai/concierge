# Contributing to Concierge

We're committed to an open development process and appreciate every
contribution — bug fixes, new game support, documentation improvements, or
just spreading the word. Concierge is a young project, which means small
contributions land fast and make a visible difference.

Not sure where to start? [Open an issue](https://github.com/msmfai/concierge/issues/new)
and tell us what you'd like to do.

## Building

```sh
nix develop          # devshell with the toolchain and helper tools
cargo build          # or from any Rust toolchain (needs bsdtar on PATH)
```

## Testing

```sh
cargo test           # the hermetic suite — no games, no network required
cargo clippy --workspace
cargo fmt --check
```

Live tests (real game installs, Nexus, CrossOver) are opt-in via environment
variables and skip cleanly when unset — e.g. `CONCIERGE_FO4_DATA` points a
BA2/ESP test at a real Fallout 4 `Data` directory.

## Code style

- `rustfmt` formatting; `clippy` clean.
- Comments and docs describe the code as it stands — what it does and why the
  design is what it is. No change history, no TODO-narration.
- Game knowledge lives behind the `GameAdapter` trait (see
  `crates/concierge-core/src/game.rs`). Core stays game-agnostic: adding a
  game means adding a crate and a registry row, never editing core dispatch.
- Every behavioural change carries a test. Hermetic tests craft their own
  fixtures (see `crates/concierge/tests/` for the pattern).

## Adding a game

The fastest path is data: a loose-file game is one row in
`concierge-filedrop`; anything with an ordered activation registry can start
as a `[game.custom]` block in a manifest. A dedicated adapter crate earns its
keep when the game needs lints, install options, or a bespoke merge step.

## Contribution terms

Concierge is licensed under GPL-3.0-or-later (see [LICENSE](LICENSE)). By
submitting a contribution you certify you have the right to contribute it
([Developer Certificate of Origin](https://developercertificate.org/) — sign
off with `git commit -s`), and you grant the project maintainer a perpetual,
worldwide, non-exclusive, no-charge, royalty-free, irrevocable copyright
license to reproduce, prepare derivative works of, publicly display, publicly
perform, sublicense, and distribute your contribution and such derivative
works. You retain copyright in your work, and it remains available to
everyone under the GPL.
