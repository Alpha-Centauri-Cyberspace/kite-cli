# Contributing to Kite CLI

Thanks for your interest! This is the client side of [Kite](https://github.com/Alpha-Centauri-Cyberspace), a webhook delivery network. The CLI is open source (MIT) and we welcome PRs.

## Local dev

```bash
cargo build                   # debug build of the `kite` binary
cargo test                    # unit + integration tests
cargo fmt --all --check       # style
cargo clippy --all-targets -- -D warnings
bash scripts/generate-cli-docs.sh  # regenerate docs/COMMANDS.md from --help
```

## Protocol changes

The wire format lives in [`kite-protocol`](https://github.com/Alpha-Centauri-Cyberspace/kite-protocol). If your change needs a new message or extension, open a PR there first, publish a new version, then bump the dep in `Cargo.toml` here.

## Releases

The crate version in `Cargo.toml` is the release source of truth. The package is
marked `publish = false` because the crates.io name belongs to an unrelated
project; Kite CLI releases only through GitHub Releases, the download mirror,
and Homebrew.

A maintainer first merges a reviewed version bump, creates a protected `vX.Y.Z`
tag on that merged commit, and manually dispatches `auto-release.yml` with that
tag selected as the workflow ref. The approval-gated workflow:

1. Validates that `Cargo.toml`, `Cargo.lock`, and the `vX.Y.Z` tag agree.
2. Confirms the tag is reachable from `main` and builds that exact source.
3. Builds only the supported darwin-arm64 and linux-x86_64 release tarballs.
4. Validates and uploads immutable archives and a versioned manifest.
5. Publishes a GitHub Release with the tarballs and SHA256 sidecars attached.
6. Advances the `latest.json`
   pointer to Cloudflare R2.
7. Allows a separate, approval-gated `publish-homebrew.yml` run to validate the
   versioned manifest and open a `Formula/kite.rb` pull request in
   [`homebrew-kite`](https://github.com/Alpha-Centauri-Cyberspace/homebrew-kite).

Both release workflows must be dispatched with the existing tag selected as the
workflow ref. The tag's version must match the checked-in package metadata; the
workflows never create tags, invent versions, or push directly to the Homebrew
tap's default branch.

## Filing issues

Bugs and feature requests welcome. Please include OS/arch, `kite --version`, and minimal reproduction steps.

## Code of conduct

Be kind, assume good faith, no harassment. The maintainers reserve the right to moderate.

## License

By contributing you agree your contributions are licensed under MIT.
