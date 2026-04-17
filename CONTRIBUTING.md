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

`main` pushes that touch `src/**`, `Cargo.toml`, or `Cargo.lock` trigger `auto-release.yml`, which:

1. Tags the next `v0.1.x`.
2. Builds darwin-arm64 + linux-x86_64 release tarballs.
3. Publishes a GitHub Release with the tarballs attached.
4. Uploads the tarballs, SHA256s, and a `manifest.json` to Cloudflare R2.
5. Triggers `publish-homebrew.yml`, which updates `Formula/kite.rb` in [`homebrew-kite`](https://github.com/Alpha-Centauri-Cyberspace/homebrew-kite).

## Filing issues

Bugs and feature requests welcome. Please include OS/arch, `kite --version`, and minimal reproduction steps.

## Code of conduct

Be kind, assume good faith, no harassment. The maintainers reserve the right to moderate.

## License

By contributing you agree your contributions are licensed under MIT.
