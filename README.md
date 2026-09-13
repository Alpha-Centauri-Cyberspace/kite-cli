# Kite CLI

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)
[![CI](https://github.com/Alpha-Centauri-Cyberspace/kite-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/Alpha-Centauri-Cyberspace/kite-cli/actions/workflows/ci.yml)

`kite` is a universal webhook adapter CLI. It streams, proxies, and replays webhook events through the Kite delivery network — turning any inbound webhook source (GitHub, Stripe, custom providers) into a reliable, replayable event stream you can wire into local dev environments, scripts, or long-running consumers.

## Install

### Homebrew (recommended)

```bash
brew tap alpha-centauri-cyberspace/kite
brew install kite
```

Homebrew packages are currently available for macOS on Apple Silicon and Linux
on x86_64 with glibc 2.34 or newer.

### Prebuilt binary

Download the archive and matching `.sha256` file for the latest release from
[GitHub Releases](https://github.com/Alpha-Centauri-Cyberspace/kite-cli/releases/latest):

| Platform | Release asset |
|---|---|
| macOS, Apple Silicon (`arm64`) | `kite-darwin-arm64.tar.gz` |
| Linux, x86_64 (glibc 2.34+) | `kite-linux-x86_64.tar.gz` |

Verify the checksum before extracting the archive. For example, on Linux x86_64:

```bash
curl -fLO https://github.com/Alpha-Centauri-Cyberspace/kite-cli/releases/latest/download/kite-linux-x86_64.tar.gz
curl -fLO https://github.com/Alpha-Centauri-Cyberspace/kite-cli/releases/latest/download/kite-linux-x86_64.tar.gz.sha256
sha256sum --check kite-linux-x86_64.tar.gz.sha256
tar -xzf kite-linux-x86_64.tar.gz
sudo install -m 0755 kite /usr/local/bin/kite
```

> [!IMPORTANT]
> Kite does not publish this CLI to crates.io or as a public container image.
> The crates.io package named `kite-cli` is an unrelated project. Use Homebrew,
> a verified GitHub release asset, or build this repository from source.

### From source

The repository pins Rust in `rust-toolchain.toml`; `cargo`/`rustup` will select that toolchain automatically.

```bash
git clone https://github.com/Alpha-Centauri-Cyberspace/kite-cli
cd kite-cli
cargo build --locked --release
./target/release/kite --help
```

The checked-in `Dockerfile` is only a convenience for building this source tree locally. It uses the same pinned Rust toolchain and is not published to a container registry:

```bash
docker build -t kite-cli-source .
docker run --rm kite-cli-source --help
```

## Quick start

```bash
kite login                   # device-auth flow
kite stream                  # follow events for your team
kite proxy --target http://localhost:3000/webhooks
kite run --manifest kite.json
```

Full command reference: [`docs/COMMANDS.md`](./docs/COMMANDS.md).

## Project layout

```
src/         Rust sources for the `kite` binary
tests/       Integration tests
docs/        Generated command reference and user docs
scripts/     Helper scripts (doc generation, etc.)
```

## Protocol

Wire format lives in the [`kite-protocol`](https://github.com/Alpha-Centauri-Cyberspace/kite-protocol) crate, published to crates.io. The CLI pins a compatible minor version — breaking protocol changes require a coordinated release.

## Contributing

See [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## License

MIT — see [`LICENSE`](./LICENSE).
