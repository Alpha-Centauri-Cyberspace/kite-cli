<div align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://getkite.sh/logo-on-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://getkite.sh/logo-on-light.svg">
    <img alt="Kite" src="https://getkite.sh/logo-on-dark.svg" width="260">
  </picture>

  <h3>Event delivery for developers and AI agents</h3>

  <p>
    <a href="https://getkite.sh"><img alt="Website" src="https://img.shields.io/badge/getkite.sh-00ff9d?style=flat-square&labelColor=0a0a0f"></a>
    <a href="https://getkite.sh/docs"><img alt="Docs" src="https://img.shields.io/badge/docs-00d4ff?style=flat-square&labelColor=0a0a0f"></a>
    <a href="https://crates.io/crates/kite-cli"><img alt="crates.io" src="https://img.shields.io/crates/v/kite-cli?color=00ff9d&labelColor=0a0a0f&style=flat-square"></a>
    <a href="https://github.com/Alpha-Centauri-Cyberspace/kite-cli/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/Alpha-Centauri-Cyberspace/kite-cli/actions/workflows/ci.yml/badge.svg"></a>
    <a href="./LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-e4e4e7?style=flat-square&labelColor=0a0a0f"></a>
  </p>
</div>

---

`kite` is the universal webhook adapter CLI — one binary that turns GitHub, Stripe, or any HTTP source into a reliable, normalized, signed event stream you can pipe, proxy, or replay from your terminal.

```
$ curl -fsSL getkite.sh/install | sh
```

## Why Kite

- **Managed endpoints.** No ngrok URL to babysit. Kite hosts the public ingress and issues you stable URLs per source.
- **CloudEvents everywhere.** Every payload is normalized to [CloudEvents](https://cloudevents.io) before it hits your code — one shape across all providers.
- **Signatures baked in.** HMAC verification happens server-side. Your local handler never sees an unsigned request.
- **Replay from the DLQ.** Failed deliveries are queued, not dropped. Retry from the command line.
- **Works with anything.** Stream to stdout, proxy to localhost, fan-out to multiple targets, or run a `kite.json` manifest.

## Install

**Homebrew** (macOS, Linux)
```
$ brew install alpha-centauri-cyberspace/kite/kite
```

**Cargo**
```
$ cargo install kite-cli
```

**Docker**
```
$ docker pull ghcr.io/alpha-centauri-cyberspace/kite-cli:latest
```

**Install script** (macOS, Linux)
```
$ curl -fsSL getkite.sh/install | sh
```

**From source**
```
$ git clone https://github.com/Alpha-Centauri-Cyberspace/kite-cli
$ cd kite-cli && cargo build --release
$ ./target/release/kite --help
```

## 60-second demo

```
$ kite login                       # browser device-auth flow
$ kite stream --json               # every event as CloudEvent JSON, streamed live
$ kite proxy --target http://localhost:3000/webhooks
```

In another terminal, fire a test event at the URL Kite gave you — it lands on `localhost:3000` with the original headers and a verified signature.

## What `kite` can do

| Command            | What it does                                                        |
| ------------------ | ------------------------------------------------------------------- |
| `kite stream`      | Stream events to stdout (filter by source, type, importance)        |
| `kite proxy`       | Forward events to a local HTTP server, with per-source routes       |
| `kite listen`      | Deliver to a Unix socket or exec a handler per event                |
| `kite run`         | Run a `kite.json` manifest — declarative routing + filters          |
| `kite retry`       | Replay events from the dead-letter queue                            |
| `kite login`       | Device-auth against `getkite.sh`                                    |
| `kite github`      | Install a signed GitHub webhook on a repo in one command            |
| `kite endpoints`   | List, create, and deactivate ingress endpoints                      |
| `kite keys`        | Manage API keys (scopes, expiry, rotation)                          |
| `kite skill`       | Install, publish, and search the agent skill registry               |
| `kite logs`        | Tail persisted event logs                                           |
| `kite status`      | Team, quota, and billing status at a glance                         |
| `kite update`      | Self-update to the latest release                                   |

Full reference: [`docs/COMMANDS.md`](./docs/COMMANDS.md) · [`getkite.sh/docs/reference/cli`](https://getkite.sh/docs/reference/cli).

## Recipes

**Proxy GitHub webhooks to your dev server**
```
$ kite github install --repo my-org/my-repo
$ kite proxy --source github --target http://localhost:3000/webhooks/github
```

**Fan out by source to multiple local services**
```
$ kite proxy \
    --route github=http://localhost:3001/gh \
    --route stripe=http://localhost:3002/stripe \
    --target  http://localhost:3000/fallback
```

**Run a manifest**
```jsonc
// kite.json
{
  "routes": [
    { "source": "github", "target": "http://localhost:3000/gh" },
    { "source": "stripe", "target": "http://localhost:3000/stripe" }
  ]
}
```
```
$ kite run --manifest kite.json
```

**Replay failed deliveries**
```
$ kite retry --source stripe --target http://localhost:3000/stripe
```

## The Kite ecosystem

- **[kite-protocol](https://github.com/Alpha-Centauri-Cyberspace/kite-protocol)** — shared Rust wire format, published to crates.io. The CLI and server pin against it.
- **[homebrew-kite](https://github.com/Alpha-Centauri-Cyberspace/homebrew-kite)** — Homebrew tap for the `kite` binary.
- **[kite-mesh](https://github.com/Alpha-Centauri-Cyberspace/kite-mesh)** — pre-alpha P2P capability discovery layer for AI agents.
- **[kite-agent-testing](https://github.com/Alpha-Centauri-Cyberspace/kite-agent-testing)** — end-to-end integration harness for the Kite ecosystem.

## Contributing

See [`CONTRIBUTING.md`](./CONTRIBUTING.md). Issues and PRs welcome — especially new provider integrations and manifest examples.

## License

MIT — see [`LICENSE`](./LICENSE).

---

<div align="center">
  <sub>
    <a href="https://getkite.sh">getkite.sh</a> ·
    <a href="https://github.com/Alpha-Centauri-Cyberspace">github</a> ·
    <a href="https://getkite.sh/docs">docs</a>
  </sub>
</div>
