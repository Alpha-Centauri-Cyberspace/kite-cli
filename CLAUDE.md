# kite-cli — agent guidance

Universal webhook adapter CLI written in Rust. User-facing reference is `README.md` and `docs/COMMANDS.md` (the latter is auto-generated from clap help via `scripts/generate-cli-docs.sh`).

## Cross-repo dependency: kite-plugin SKILL.md

**Whenever you change the CLI surface, update the public Claude Code skill that documents it.**

The skill lives in a separate repo and is published to Claude Code users via `/plugin install kite@kite-plugins`:

- Repo: https://github.com/Alpha-Centauri-Cyberspace/kite-plugin
- Local clone (if present): `../kite-plugin/`
- Skill file: `kite-plugin/skills/kite/SKILL.md`
- Plugin manifest: `kite-plugin/.claude-plugin/plugin.json` (bump `version` to ship the update)

### What counts as a "CLI surface change"

Any of the following — anything a user could observe by running `kite --help` or following a tutorial:

- New top-level subcommand (anything added under `src/commands/`)
- New / removed / renamed flag on an existing subcommand
- Change to flag semantics, defaults, or accepted values
- Change to output format (`stream`, `proxy` headers, `--json`/`--compact` shape)
- Change to the auth flow (`kite login` server URL, device-auth behavior)
- Change to env vars the CLI reads (e.g. `GITHUB_TOKEN`, `CARGO_TARGET_DIR`)
- Change to install paths or platform support

If `docs/COMMANDS.md` or `README.md` changed in your diff, the SKILL almost certainly needs an update too.

### How to update the skill

1. Run `./scripts/generate-cli-docs.sh` so `docs/COMMANDS.md` reflects the current clap help.
2. Open `../kite-plugin/skills/kite/SKILL.md` and update the affected sections (commands table, common patterns, filtering, troubleshooting). Keep examples runnable.
3. Bump `version` in `../kite-plugin/.claude-plugin/plugin.json` (semver — patch for clarifications, minor for new commands/flags, major for breaking removals).
4. Commit + PR the kite-plugin repo separately. The plugin repo enforces admin-only writes via ruleset, so non-admin contributors should open a PR.
5. Tie the kite-cli release and the kite-plugin release together — don't ship one without the other. Reference the kite-cli PR/version in the kite-plugin commit so the connection is obvious.

### Hook reinforcement

This repo ships a Claude Code hook at `.claude/hooks/skill-sync-reminder.sh` that fires when an agent edits CLI surface files (`src/commands/**`, `src/main.rs`, `docs/COMMANDS.md`) and emits a reminder. The hook is configured in `.claude/settings.json` and is the safety net — this CLAUDE.md is the primary contract.

## Commit style

Conventional commits (`feat:`, `fix:`, `chore:`, `docs:`). The CI release workflow keys off Cargo version bumps in `Cargo.toml`.
