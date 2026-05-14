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

### Cargo

```bash
cargo install kite-cli
```

### Docker

```bash
docker pull ghcr.io/alpha-centauri-cyberspace/kite-cli:latest
```

### From source

```bash
git clone https://github.com/Alpha-Centauri-Cyberspace/kite-cli
cd kite-cli
cargo build --release
./target/release/kite --help
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
