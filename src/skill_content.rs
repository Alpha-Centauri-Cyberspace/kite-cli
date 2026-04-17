pub const SKILL_MD: &str = r#"---
name: kite
description: Configure and use Kite webhook delivery tunnel for receiving, filtering, enriching, and consuming webhook events. Use when an agent needs to react to external events (GitHub, Stripe, deploy hooks, etc).
---

# Kite Skill

Kite is a webhook delivery tunnel for agentic notifications. It receives webhooks from external sources, fans them out over WebSocket connections to CLI clients, and supports filtering, enrichment, scoring, and local delivery.

## Quick Start

```bash
# 1. Authenticate
kite login

# 2. Create a webhook endpoint for a source
kite endpoints create --source github --repo owner/repo

# 3. Stream events to stdout (inspect payloads)
kite stream --source github --json

# 4. Proxy events to a local HTTP server
kite proxy --target http://localhost:3000/webhooks

# 5. Run with a kite.json manifest (filters, enrichment, exec)
kite run --manifest kite.json
```

## GitHub Auto-Setup

Kite can register webhooks on GitHub automatically:

```bash
# Auto-register (uses gh CLI or GITHUB_TOKEN)
kite endpoints create --source github --repo myorg/myrepo

# With custom events
kite endpoints create --source github --repo myorg/myrepo --events push,release

# Replace existing webhook
kite endpoints create --source github --repo myorg/myrepo --force

# Explicit token (for CI/agents)
kite endpoints create --source github --repo myorg/myrepo --github-token ghp_xxx
```

Auth order: gh CLI → GITHUB_TOKEN env → --github-token flag.

## Configuration

Kite is configured via a `kite.json` manifest. Example with all major features:

```json
{
  "source": "github",
  "filters": [
    { "field": "type", "op": "eq", "value": "com.github.pull_request" },
    { "field": "data.action", "op": "in", "values": ["opened", "synchronize", "reopened"] }
  ],
  "enrichment": {
    "script": "./enrich.sh"
  },
  "scoring": {
    "script": "./score.sh",
    "threshold": 0.5
  },
  "sink": {
    "type": "exec",
    "command": "./handle-pr.sh"
  },
  "queue": {
    "enabled": true,
    "path": "./kite-queue.db"
  }
}
```

### Filter operators

- `eq` — exact match
- `neq` — not equal
- `contains` — substring or array contains
- `in` — field value is in a list
- `gt`, `lt`, `gte`, `lte` — numeric comparisons
- `exists` — field is present

### Sink types

- `exec` — run a command per event (event JSON piped to stdin)
- `http` — POST event to a URL
- `socket` — write to a Unix domain socket

## Event Queue

The local SQLite queue persists events for durable delivery and replay.

```bash
kite queue list                         # list queued events
kite queue list --status failed         # filter by status
kite queue list --source github         # filter by source
kite queue list --importance high       # filter by importance
kite queue list --since 24h             # events from last 24h
kite queue show <seq>                   # full details for one event
kite queue replay --target http://localhost:3000 --status failed  # replay failed
kite queue replay --seq 42 --target http://localhost:3000         # replay one
kite queue flush 7d                     # delete events older than 7 days
kite queue stats                        # counts grouped by status
```

Event statuses: `pending`, `ready`, `delivered`, `failed`, `filtered`, `enriching`

## Delivery Cursors

Use `--client-id` to track delivery position durably. Kite remembers the last delivered event for each client ID, so reconnecting agents resume from where they left off.

```bash
kite stream --client-id my-agent-v1 --source github
kite proxy --client-id my-agent-v1 --target http://localhost:3000
```

## Writing Enrichment Scripts

Enrichment scripts receive the CloudEvent as JSON on stdin and must write enriched JSON to stdout. Exit non-zero to drop the event.

```bash
#!/usr/bin/env bash
# enrich.sh — add a repo_size label
set -euo pipefail

event=$(cat)
repo=$(echo "$event" | jq -r '.data.repository.full_name')
size=$(gh api "repos/$repo" --jq '.size' 2>/dev/null || echo "0")

echo "$event" | jq --argjson size "$size" '.data.kite_repo_size = $size'
```

Scoring scripts follow the same convention but must output a single float between 0 and 1.

## Common Patterns

### PR Review Agent

```json
{
  "source": "github",
  "filters": [
    { "field": "type", "op": "eq", "value": "com.github.pull_request" },
    { "field": "data.action", "op": "eq", "value": "opened" }
  ],
  "sink": {
    "type": "exec",
    "command": "claude -p 'Review this PR and post a comment' --input-format json"
  }
}
```

### Deploy Monitor

```json
{
  "source": "github",
  "filters": [
    { "field": "type", "op": "eq", "value": "com.github.workflow_run" },
    { "field": "data.workflow_run.conclusion", "op": "eq", "value": "failure" }
  ],
  "sink": {
    "type": "http",
    "url": "http://localhost:9000/deploy-alert"
  },
  "queue": {
    "enabled": true,
    "path": "./deploy-queue.db"
  }
}
```

## CLI Reference

| Command | Description |
|---|---|
| `kite login` | Authenticate via device auth flow |
| `kite stream` | Stream events to stdout (--json for full payload) |
| `kite proxy` | Forward events to a local HTTP server |
| `kite listen` | Listen via Unix socket or exec |
| `kite run` | Run with a kite.json manifest |
| `kite retry` | Retry failed events from DLQ |
| `kite endpoints list` | List webhook endpoints |
| `kite endpoints create` | Create/rotate endpoint credentials (--repo for auto-setup) |
| `kite endpoints deactivate` | Deactivate an endpoint |
| `kite keys list` | List API keys |
| `kite keys create` | Create a new API key |
| `kite keys revoke` | Revoke an API key |
| `kite skill install` | Install an agent skill |
| `kite skill list` | List installed skills |
| `kite skill export` | Export the kite SKILL.md to agent skill directories |
| `kite queue list` | List queued events |
| `kite queue show` | Show details for a single event |
| `kite queue replay` | Replay events to a target URL |
| `kite queue flush` | Delete old events |
| `kite queue stats` | Show event counts by status |
| `kite logs` | View persisted event logs |
| `kite status` | Show dashboard-equivalent status |
| `kite update` | Update the kite binary |
"#;
