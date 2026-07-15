# Concierge v0.1.0

> [!WARNING]
> **Alpha release.** Features and game compatibility are still evolving.

Concierge creates reproducible modded game installations from simple text
pack definitions. It prepares each pack in a separate copy of the game, so the
original installation remains untouched, and provides the complete workflow
for finding files, reviewing changes, installing, validating, launching, and
rolling back a pack.

## What Concierge does

- Keeps a mod pack reproducible and reviewable as a text file.
- Downloads requested files, verifies their integrity, and reuses cached
  downloads across packs.
- Previews planned changes before modifying a deployment.
- Builds isolated game installations without changing the original game.
- Resolves installation and load order, then checks the result for missing
  requirements, conflicts, unexpected disk changes, and game limits.
- Supports multiple packs per game, rollback to earlier states, and complete
  removal of a deployment.
- Provides both a desktop app and command-line interface in every release
  archive.
- Allows an assistant to manage a pack through a restricted workspace that
  cannot write outside the pack's permitted locations.

Concierge supports dozens of games, although the depth of validation and
automation varies by game while the project is in alpha.

## Downloads

Choose the archive attached to this release for your operating system. Each
archive includes both the desktop app and command-line interface.

## Known limitations

- Some game workflows have received substantially more real-world testing
  than others.
- Advanced validation and conflict views depend on the information available
  for each game.
- Installers that require unusual interactive choices may need additional pack
  configuration.
